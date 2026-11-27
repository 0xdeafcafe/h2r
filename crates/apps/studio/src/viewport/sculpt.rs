//! The sculpt brush in the viewport (docs/editor/modeling.md,
//! "Sculpting"). While Modeling's brush is on and a structure cluster is
//! in edit mode, the cursor's hit on the edit mesh carries the brush ring
//! and its falloff rings; a left drag collects the stroke's path (a point
//! each quarter radius), drawn as it goes, and the release applies it as
//! one `MeshOp::Sculpt`, one undo step. The kernel does the work
//! (`mesh_edit::Stroke`); this turns the mouse into points of the mesh.
use dear_imgui_rs::{MouseButton, Ui};
use geometry_edit::TargetKind;
use geometry_edit::mesh_edit::MeshOp;

use crate::app::modeling::ModelAction;
use crate::app::{Action, App};

/// Segments of a drawn ring.
const SEGMENTS: usize = 48;
/// The falloff rings: where the weight is each of these.
const RINGS: [f32; 3] = [0.75, 0.5, 0.25];
const RIM: [f32; 4] = [1.0, 0.85, 0.2, 1.0];
const PATH: [f32; 4] = [1.0, 1.0, 1.0, 0.8];

/// The stroke being dragged.
#[derive(Default)]
pub struct SculptBrush {
    points: Vec<[f32; 3]>,
    drawing: bool,
}

impl SculptBrush {
    fn reset(&mut self) -> (bool, bool) {
        self.points.clear();
        self.drawing = false;
        (false, false)
    }

    /// Draws the brush and runs its stroke. Returns whether the mouse is
    /// over the mesh with the brush on, and whether a stroke is being
    /// dragged, so the camera and picking leave the mouse alone.
    pub fn run(&mut self, ui: &Ui, app: &mut App, origin: [f32; 2], size: [f32; 2], aspect: f32) -> (bool, bool) {
        let settings = app.modeling.brush.clone();
        let target = app.modeling.session.active.filter(|t| t.kind == TargetKind::Cluster);
        let (Some(target), true) = (target, settings.on) else {
            return self.reset();
        };
        let Some(mesh) = app.modeling.session.mesh(target) else {
            return self.reset();
        };
        let camera = *app.session.camera();
        let mouse = ui.io().mouse_pos();
        let uv = [(mouse[0] - origin[0]) / size[0], (mouse[1] - origin[1]) / size[1]];
        let inside = (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]);
        let ray = camera.ray(uv[0], uv[1], aspect);
        let hit = inside
            .then(|| mesh.raycast(ray.origin, ray.dir))
            .flatten()
            .map(|(t, _)| [0, 1, 2].map(|k| ray.origin[k] + ray.dir[k] * t));
        let alt = ui.io().key_alt();

        if !self.drawing && !alt && ui.is_mouse_clicked(MouseButton::Left) && let Some(h) = hit {
            self.points = vec![h];
            self.drawing = true;
        }
        if self.drawing && ui.is_mouse_down(MouseButton::Left) && let Some(h) = hit {
            let last = *self.points.last().expect("a stroke starts with a point");
            let step = (h[0] - last[0]).hypot(h[1] - last[1]);
            if step > settings.radius / 4.0 {
                self.points.push(h);
            }
        }
        if self.drawing && !ui.is_mouse_down(MouseButton::Left) {
            self.drawing = false;
            let points = std::mem::take(&mut self.points);
            app.dispatch(Action::Model(ModelAction::Seal));
            app.dispatch(Action::Model(ModelAction::Apply {
                target,
                ops: vec![MeshOp::Sculpt(settings.stroke(points))],
                merge: None,
            }));
        }

        let px = |p: [f32; 3]| {
            camera
                .project(p, aspect)
                .map(|s| [origin[0] + s[0] * size[0], origin[1] + s[1] * size[1]])
        };
        let list = ui.get_window_draw_list();
        let _clip = list.push_clip_rect(origin, [origin[0] + size[0], origin[1] + size[1]], true);
        let ring = |c: [f32; 3], r: f32, colour: [f32; 4], thickness: f32| {
            let at = |k: usize| {
                let a = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                px([c[0] + r * a.cos(), c[1] + r * a.sin(), c[2]])
            };
            for k in 0..SEGMENTS {
                if let (Some(a), Some(b)) = (at(k), at(k + 1)) {
                    list.add_line(a, b, colour).thickness(thickness).build();
                }
            }
        };
        for pair in self.points.windows(2) {
            if let (Some(a), Some(b)) = (px(pair[0]), px(pair[1])) {
                list.add_line(a, b, PATH).thickness(2.0).build();
            }
        }
        if let Some(c) = hit {
            ring(c, settings.radius, RIM, 2.0);
            for w in RINGS {
                let r = settings.radius * settings.falloff.radius_at(w);
                ring(c, r, [RIM[0], RIM[1], RIM[2], 0.25 + 0.6 * w], 1.0);
            }
            if let Some(p) = px(c) {
                list.add_circle(p, 2.5, RIM).filled(true).build();
            }
        }
        (inside && hit.is_some(), self.drawing)
    }
}
