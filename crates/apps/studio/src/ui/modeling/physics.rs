use dear_imgui_rs::{TreeNodeFlags, Ui};
use map::TagId;
use tags::collision::{PHMO, PhysicsModel, ShapeType, shape_edit::ShapeEdit};

use crate::App;
use crate::app::{Action, TagAction};

type Revision = (u64, u64);

#[derive(Default)]
pub(super) struct Panel {
    tag: Option<TagId>,
    shape: usize,
    field: usize,
    draft: Option<(Revision, ShapeEdit)>,
    loaded: Option<(Revision, TagId, Result<PhysicsModel, String>)>,
}

impl Panel {
    pub(super) fn draw(&mut self, ui: &Ui, app: &mut App, actions: &mut Vec<Action>) {
        if !ui.collapsing_header("Physics shapes", TreeNodeFlags::NONE) {
            return;
        }
        app.sync_tags();
        if app.browsing().is_some() {
            ui.text_disabled("Open the map for editing.");
            return;
        }
        let tags = app
            .tags
            .tags()
            .iter()
            .filter(|t| t.group == PHMO && t.size != 0);
        let Some(tag) = tags
            .clone()
            .find(|t| Some(t.id) == self.tag)
            .or_else(|| tags.clone().next())
        else {
            ui.text_disabled("This map has no local physics models.");
            return;
        };
        let mut picked = tag.id;
        if let Some(_combo) = ui.begin_combo("Physics model##phmo-model", &tag.name) {
            for tag in tags {
                if ui
                    .selectable_config(format!("{}##phmo-{}", tag.name, tag.id.0))
                    .selected(Some(tag.id) == self.tag)
                    .build()
                {
                    picked = tag.id;
                }
            }
        }
        if self.tag != Some(picked) {
            self.tag = Some(picked);
            self.shape = 0;
            self.field = 0;
            self.draft = None;
        }

        let revision = (app.session.world_rev(), app.tags.rev());
        if self
            .loaded
            .as_ref()
            .is_some_and(|(r, tag, _)| *r != revision || *tag != picked)
        {
            self.loaded = None;
        }
        let (_, _, loaded) = self
            .loaded
            .get_or_insert_with(|| (revision, picked, app.physics_model(picked)));
        let model = match loaded {
            Ok(model) => model,
            Err(why) => {
                ui.text_disabled(why.as_str());
                return;
            }
        };
        let shapes: Vec<_> = [
            (ShapeType::Sphere, model.spheres.len()),
            (ShapeType::Pill, model.pills.len()),
            (ShapeType::Box, model.boxes.len()),
            (ShapeType::Polyhedron, model.polyhedra.len()),
        ]
        .into_iter()
        .flat_map(|(kind, count)| (0..count).map(move |i| (kind, i)))
        .collect();
        if shapes.is_empty() {
            ui.text_disabled("This model has no editable shapes.");
            return;
        }
        self.shape = self.shape.min(shapes.len() - 1);
        let label = |&(kind, i): &(ShapeType, usize)| format!("{kind:?} {i}");
        if let Some(_combo) = ui.begin_combo("Shape##phmo-shape", label(&shapes[self.shape])) {
            for (i, shape) in shapes.iter().enumerate() {
                if ui
                    .selectable_config(label(shape))
                    .selected(i == self.shape)
                    .build()
                {
                    self.shape = i;
                    self.field = 0;
                    self.draft = None;
                }
            }
        }
        let (kind, index) = shapes[self.shape];
        let fields: Vec<(String, ShapeEdit)> = match kind {
            ShapeType::Sphere => {
                let shape = model.spheres[index];
                vec![
                    ("Radius".into(), ShapeEdit::Radius(shape.radius)),
                    (
                        "Translation".into(),
                        ShapeEdit::Translation(shape.transform.translation),
                    ),
                    (
                        "Rotation".into(),
                        ShapeEdit::Rotation(shape.transform.rotation),
                    ),
                ]
            }
            ShapeType::Pill => {
                let shape = model.pills[index];
                vec![
                    ("Radius".into(), ShapeEdit::Radius(shape.radius)),
                    ("Bottom".into(), ShapeEdit::Bottom(shape.bottom)),
                    ("Top".into(), ShapeEdit::Top(shape.top)),
                ]
            }
            ShapeType::Box => {
                let shape = model.boxes[index];
                vec![
                    (
                        "Half extents".into(),
                        ShapeEdit::HalfExtents(shape.half_extents),
                    ),
                    ("Radius".into(), ShapeEdit::Radius(shape.radius)),
                    (
                        "Translation".into(),
                        ShapeEdit::Translation(shape.transform.translation),
                    ),
                    (
                        "Rotation".into(),
                        ShapeEdit::Rotation(shape.transform.rotation),
                    ),
                ]
            }
            ShapeType::Polyhedron => model.polyhedra[index]
                .vertices
                .iter()
                .enumerate()
                .map(|(index, &position)| {
                    (
                        format!("Vertex {index}"),
                        ShapeEdit::Vertex { index, position },
                    )
                })
                .collect(),
            _ => unreachable!("only editable shapes are listed"),
        };
        if fields.is_empty() {
            ui.text_disabled("This hull has no vertices.");
            return;
        }
        self.field = self.field.min(fields.len() - 1);
        if let Some(_combo) = ui.begin_combo("Field##phmo-field", &fields[self.field].0) {
            for (i, (name, _)) in fields.iter().enumerate() {
                if ui.selectable_config(name).selected(i == self.field).build() {
                    self.field = i;
                    self.draft = None;
                }
            }
        }
        let source = &fields[self.field].1;
        // Include the map revision so an edit buffer never survives a map
        // change even if both maps use the same datum and shape index.
        if self.draft.as_ref().is_none_or(|(r, _)| *r != revision) {
            self.draft = Some((revision, source.clone()));
        }
        let Some((_, edit)) = self.draft.as_mut() else {
            return;
        };
        let xyz = |row: usize, point: &mut [f32; 3]| {
            for (axis, value) in ["X", "Y", "Z"].into_iter().zip(point) {
                let label = if matches!(source, ShapeEdit::Rotation(_)) {
                    format!("{axis} (row {})##phmo-{row}-{axis}", row + 1)
                } else {
                    format!("{axis}##phmo-{axis}")
                };
                ui.input_float(label, value);
            }
        };
        match edit {
            ShapeEdit::Radius(value) => {
                ui.input_float("Radius##phmo-value", value);
            }
            ShapeEdit::Rotation(rows) => {
                for (row, values) in rows.iter_mut().enumerate() {
                    xyz(row, values);
                }
            }
            ShapeEdit::Translation(point)
            | ShapeEdit::HalfExtents(point)
            | ShapeEdit::Bottom(point)
            | ShapeEdit::Top(point)
            | ShapeEdit::Vertex {
                position: point, ..
            } => xyz(0, point),
        }
        if ui.button("Apply shape edit") && *edit != *source {
            actions.push(Action::Tag(TagAction::SetPhysicsShape {
                tag: picked,
                kind,
                index,
                edit: edit.clone(),
            }));
        }
    }
}
