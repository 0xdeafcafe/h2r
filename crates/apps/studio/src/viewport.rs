//! The 3D viewport: the scenario crate's offscreen wgpu renderer
//! ([`scenario_edit::render::Viewport`]: the sky, the textured structure
//! BSP, the placed objects' models and a glyph per placement without one)
//! registered with `dear-imgui-wgpu` as an external texture and drawn as
//! an ImGui image, with picking (Ctrl/Cmd adds to the selection), the
//! game-editor camera of [`crate::camera`], an ImGuizmo move, rotate or
//! scale gizmo on the selection with grid and angle snapping, and placing
//! a new entry where the cursor meets the level.
pub(crate) mod mirror;

use std::collections::BTreeSet;
use dear_imgui_rs::{Key, MouseButton, TextureId, Ui};
use dear_imgui_wgpu::{ExternalTextureId, WgpuRenderer};
use dear_imguizmo::{GuizmoExt, Mode, Operation};
use scenario_edit::render::{self, Viewport};
use scenario_edit::{BlockKind, Session, geometry};

use crate::app::{Action, App};
use crate::camera;

/// The gizmo's operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoOp {
    Translate,
    Rotate,
    /// Uniform scale, for placements with a scale (objects).
    Scale,
}

/// Snapping for gizmo drags and placing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    /// Moves go in steps of [`Self::grid_step`]; placing rounds x and y to
    /// the grid.
    pub grid: bool,
    pub grid_step: f32,
    /// Turns go in steps of [`Self::angle_step`] degrees.
    pub angle: bool,
    pub angle_step: f32,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            grid: false,
            grid_step: 0.5,
            angle: false,
            angle_step: 15.0,
        }
    }
}

/// The grid steps and angle steps the toolbar offers.
pub const GRID_STEPS: [f32; 5] = [0.1, 0.25, 0.5, 1.0, 2.0];
pub const ANGLE_STEPS: [f32; 4] = [5.0, 15.0, 45.0, 90.0];

/// `p` with x and y on the grid (z is left where the surface put it).
pub fn snap_to_grid(p: [f32; 3], step: f32) -> [f32; 3] {
    if step <= 0.0 {
        return p;
    }
    let r = |x: f32| (x / step).round() * step;
    [r(p[0]), r(p[1]), p[2]]
}

/// What the gizmo moves the primary selection by when snapping: ImGuizmo's
/// snap per operation (a step for each axis, or degrees in x).
pub fn gizmo_snap(op: GizmoOp, snap: &Snap) -> Option<[f32; 3]> {
    match op {
        GizmoOp::Translate if snap.grid => Some([snap.grid_step; 3]),
        GizmoOp::Rotate if snap.angle => Some([snap.angle_step, 0.0, 0.0]),
        GizmoOp::Scale if snap.grid => Some([0.1; 3]),
        _ => None,
    }
}

/// What the gizmo operates for a selection: facing-only placements turn
/// about z, placements without a rotation or scale are moved instead.
pub fn gizmo_operation(op: GizmoOp, t: &scenario_edit::Transform, scalable: bool) -> Operation {
    match (op, t.rotation) {
        (GizmoOp::Translate, _) => Operation::TRANSLATE,
        (GizmoOp::Rotate, Some(scenario_edit::RotationField::Facing(_))) => Operation::ROTATE_Z,
        (GizmoOp::Rotate, Some(_)) => Operation::ROTATE,
        (GizmoOp::Rotate, None) => Operation::TRANSLATE,
        (GizmoOp::Scale, _) if scalable => Operation::SCALE_UNIFORM,
        (GizmoOp::Scale, _) => Operation::TRANSLATE,
    }
}

/// What the GPU side the viewport needs while the UI is built.
pub struct Gpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub renderer: &'a mut WgpuRenderer,
}

/// The viewport's GPU state and camera interaction.
pub struct ViewportView {
    vp: Option<Viewport>,
    texture: Option<ExternalTextureId>,
    world_rev: Option<u64>,
    markers_rev: Option<u64>,
    linked: BTreeSet<(BlockKind, usize)>,
    overlay_rev: Option<u64>,
    pub op: GizmoOp,
    /// Gizmo axes follow the placement (local) or the world.
    pub local: bool,
    pub snap: Snap,
    /// The camera controls (fly speed, easing, glides).
    pub camera: camera::Controller,
    /// The map the camera controller last saw.
    camera_world: Option<u64>,
    /// Right mouse is held over the viewport: look around and WASD/QE fly.
    flying: bool,
    /// Alt + left drag orbits.
    orbiting: bool,
    panning: bool,
    /// The next left click in the viewport places a new entry of this
    /// block there (a copy of the entry, when given).
    pub placing: Option<(BlockKind, Option<usize>)>,
    /// Where the last frame showed the image: origin and size in points.
    shown: Option<[f32; 4]>,
}

impl Default for ViewportView {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportView {
    pub fn new() -> Self {
        Self {
            vp: None,
            texture: None,
            world_rev: None,
            markers_rev: None,
            linked: BTreeSet::new(),
            overlay_rev: None,
            op: GizmoOp::Translate,
            local: false,
            snap: Snap::default(),
            camera: camera::Controller::new(),
            camera_world: None,
            flying: false,
            orbiting: false,
            panning: false,
            placing: None,
            shown: None,
        }
    }

    /// Where the last frame showed the viewport (origin and size in
    /// points), and its render target: the headless tests compare the two.
    pub fn shown(&self) -> Option<([f32; 4], &wgpu::Texture)> {
        Some((self.shown?, self.vp.as_ref()?.color_texture()?))
    }

    /// Whether the camera has the mouse (keyboard shortcuts then fly).
    pub fn looking(&self) -> bool {
        self.flying
    }

    /// Whether the uploaded map has a sky model.
    pub fn has_sky(&self) -> bool {
        self.vp.as_ref().is_some_and(Viewport::has_sky)
    }

    /// Render the scene at `size_px` and return the ImGui texture showing it.
    fn prepare(&mut self, gpu: &mut Gpu<'_>, app: &App, live: &mirror::Scene, size_px: [u32; 2]) -> Option<TextureId> {
        let vp = self.vp.get_or_insert_with(|| Viewport::new(gpu.device));
        let session = &app.session;
        // The map's textured world and models when it has them (built once
        // per map and kept on the GPU until another map opens), else the
        // grey world and glyphs.
        let world_changed = self.world_rev != Some(session.world_rev());
        if world_changed {
            match session.scene() {
                Some(scene) => vp.set_scene(gpu.device, gpu.queue, scene),
                None => vp.set_world(gpu.device, session.world().0),
            }
            self.world_rev = Some(session.world_rev());
        }
        if world_changed || self.markers_rev != Some(session.markers_rev()) || self.linked != live.linked {
            let selected = session.selection();
            match session.scene() {
                Some(scene) => vp.set_linked_placements(gpu.device, scene, session.markers(), &selected, &live.linked),
                None => vp.set_markers(
                    gpu.device,
                    &render::marker_vertices(session.markers(), &selected),
                ),
            }
            self.markers_rev = Some(session.markers_rev());
            self.linked.clone_from(&live.linked);
        }
        vp.set_live_instances(gpu.device, gpu.queue, session.source(), &live.instances);
        if self.overlay_rev != Some(app.overlay_rev()) {
            vp.set_overlay(gpu.device, &overlay_vertices(app));
            self.overlay_rev = Some(app.overlay_rev());
        }
        let resized = vp.render(gpu.device, gpu.queue, session.camera(), size_px);
        let view = vp.ui_view()?;
        match self.texture {
            Some(id) if resized => {
                if let Err(e) = gpu.renderer.update_external_texture(id, view) {
                    eprintln!("viewport texture: {e}");
                }
            }
            Some(_) => {}
            None => match gpu.renderer.register_external_texture(view) {
                Ok(id) => self.texture = Some(id),
                Err(e) => {
                    eprintln!("viewport texture: {e}");
                    return None;
                }
            },
        }
        self.texture.map(|t| t.texture_id())
    }

    /// Draw the viewport into the current window's remaining space and
    /// handle its input. Returns the actions it asks for.
    pub fn draw(&mut self, ui: &Ui, app: &mut App, gpu: &mut Gpu<'_>, actions: &mut Vec<Action>) {
        let avail = ui.content_region_avail();
        let size = [avail[0].max(64.0), avail[1].max(64.0)];
        let scale = ui.io().display_framebuffer_scale();
        let px = [
            (size[0] * scale[0]).round() as u32,
            (size[1] * scale[1]).round() as u32,
        ];
        let origin = ui.cursor_screen_pos();
        let live = mirror::Scene::shown(app);
        let Some(texture) = self.prepare(gpu, app, &live, px) else {
            ui.text_disabled("The viewport could not create its render target.");
            return;
        };
        ui.image(texture, size);
        let rect = [origin[0], origin[1], size[0], size[1]];
        self.shown = Some(rect);
        live.overlay(ui, app.session.camera(), rect);
        let hovered = ui.is_item_hovered();
        let aspect = size[0] / size[1].max(1.0);

        let (over_gizmo, using_gizmo) = self.gizmo(ui, app, origin, size, aspect);

        let io = ui.io();
        let alt = io.key_alt();
        let cmd = io.key_ctrl() || io.key_super();
        let m = io.mouse_pos();
        let uv = [(m[0] - origin[0]) / size[0], (m[1] - origin[1]) / size[1]];

        // Camera: right button looks and flies, Alt + left orbits, middle
        // pans, the wheel dollies (or sets the fly speed while flying).
        if hovered && ui.is_mouse_clicked(MouseButton::Right) {
            self.flying = true;
        }
        if !ui.is_mouse_down(MouseButton::Right) {
            self.flying = false;
        }
        if hovered && alt && !using_gizmo && ui.is_mouse_clicked(MouseButton::Left) {
            let ray = app.session.camera().ray(uv[0], uv[1], aspect);
            let pivot = app
                .session
                .selection_sphere()
                .map(|(c, _)| c)
                .or_else(|| app.session.surface_hit(&ray))
                .unwrap_or(app.session.camera().target);
            self.camera.begin_orbit(pivot);
            self.orbiting = true;
        }
        if !ui.is_mouse_down(MouseButton::Left) {
            self.orbiting = false;
        }
        if hovered && ui.is_mouse_clicked(MouseButton::Middle) {
            self.panning = true;
        }
        if !ui.is_mouse_down(MouseButton::Middle) {
            self.panning = false;
        }
        let k = |key| if ui.is_key_down(key) { 1.0 } else { 0.0 };
        let keys = if self.flying {
            [
                k(Key::D) - k(Key::A),
                k(Key::E) - k(Key::Q),
                k(Key::W) - k(Key::S),
            ]
        } else {
            [0.0; 3]
        };
        if self.camera_world != Some(app.session.world_rev()) {
            self.camera.reset();
            self.camera_world = Some(app.session.world_rev());
        }
        let input = camera::Input {
            dt: io.delta_time(),
            mouse: io.mouse_delta(),
            wheel: if hovered || self.flying {
                io.mouse_wheel()
            } else {
                0.0
            },
            fly: self.flying,
            orbit: self.orbiting,
            pan: self.panning,
            keys,
            fast: io.key_shift(),
            height: size[1],
        };
        self.camera.update(app.session.camera_mut(), &input);
        let ray = app.session.camera().ray(uv[0], uv[1], aspect);

        if self.placing.is_some() && ui.is_key_pressed(Key::Escape) {
            self.placing = None;
        }
        let clicked = hovered
            && !alt
            && !over_gizmo
            && !using_gizmo
            && ui.is_mouse_clicked(MouseButton::Left);
        // Placing: the click puts the new entry where the cursor meets the
        // level.
        if clicked && let Some((kind, like)) = self.placing {
            match app.session.surface_hit(&ray) {
                Some(at) => {
                    let at = if self.snap.grid {
                        snap_to_grid(at, self.snap.grid_step)
                    } else {
                        at
                    };
                    actions.push(Action::PlaceAt(kind, like, at));
                    self.placing = None;
                }
                None => app.error("place it on the level: nothing is under the cursor"),
            }
            return;
        }
        // Double-click focuses what is under the cursor: a placement, else
        // the surface.
        if hovered && !alt && !over_gizmo && ui.is_mouse_double_clicked(MouseButton::Left) {
            match app.session.pick(&ray) {
                Some(hit) => {
                    actions.push(Action::Select(Some(hit)));
                    actions.push(Action::FocusSelected);
                }
                None => {
                    if let Some(p) = app.session.surface_hit(&ray) {
                        let to = camera::focus_point(app.session.camera(), p, 30.0);
                        self.camera.go_to(app.session.camera_mut(), to);
                    }
                }
            }
            return;
        }
        // Left click picks; with Ctrl/Cmd it adds to or takes from the
        // selection.
        if clicked && let Some(action) = pick_action(&app.session, &ray, cmd) {
            actions.push(action);
        }
    }

    /// The gizmo on the primary selection; the rest of a multi-selection
    /// moves with it. Returns whether the mouse is over it and whether it
    /// is being dragged.
    fn gizmo(
        &mut self,
        ui: &Ui,
        app: &mut App,
        origin: [f32; 2],
        size: [f32; 2],
        aspect: f32,
    ) -> (bool, bool) {
        let camera = *app.session.camera();
        let view = geometry::flat(&camera.view());
        let proj = geometry::flat(&camera.proj(aspect));
        let mut over = false;
        let mut using = false;
        if let Some(t) = app.session.selected_transform() {
            let scale = app.session.selected_scale();
            let mut model = t.matrix();
            if self.op == GizmoOp::Scale
                && let Some(s) = scale
            {
                for v in &mut model[..12] {
                    *v *= s;
                }
            }
            let gizmo = ui.guizmo();
            gizmo.set_drawlist_window();
            gizmo.set_rect(origin[0], origin[1], size[0], size[1]);
            gizmo.set_orthographic(false);
            let op = gizmo_operation(self.op, &t, scale.is_some());
            let mode = if self.local { Mode::Local } else { Mode::World };
            let mut config = gizmo
                .manipulate_config(&view, &proj, &mut model)
                .operation(op)
                .mode(mode);
            if let Some(snap) = gizmo_snap(self.op, &self.snap) {
                config = config.snap(snap);
            }
            let changed = config.build();
            over = gizmo.is_over();
            using = gizmo.is_using();
            if changed {
                // Through the command layer, as the agent API's edits.
                let action = if op == Operation::SCALE_UNIFORM {
                    let x = [model[0], model[1], model[2]];
                    Action::GestureScale(geometry::dot(x, x).sqrt())
                } else {
                    let (pos, ypr) = geometry::decompose(&model);
                    let ypr = match t.rotation {
                        // Translating never touches the rotation.
                        _ if self.op != GizmoOp::Rotate => t.ypr(),
                        Some(_) => ypr,
                        None => [0.0; 3],
                    };
                    let pos = if self.op == GizmoOp::Rotate {
                        t.position
                    } else {
                        pos
                    };
                    Action::GestureSet { position: pos, ypr }
                };
                app.dispatch(action);
            }
        }
        if !using && app.session.in_gesture() {
            app.dispatch(Action::EndGesture);
            let what = selection_label(app);
            let verb = match self.op {
                GizmoOp::Translate => "moved",
                GizmoOp::Rotate => "turned",
                GizmoOp::Scale => "scaled",
            };
            app.info(format!("{verb} {what}"));
        }
        (over, using)
    }
}

pub(crate) fn pick_action(session: &Session, ray: &geometry::Ray, extend: bool) -> Option<Action> {
    let hit = session.pick(ray);
    if extend {
        hit.map(Action::ToggleSelect)
    } else {
        Some(Action::Select(hit))
    }
}

fn selection_label(app: &App) -> String {
    let n = app.session.selection().len();
    match (app.session.selected(), app.session.scenario()) {
        _ if n > 1 => format!("{n} placements"),
        (Some((kind, i)), Some(s)) => format!("{:?} {}", kind, s.label(kind, i)),
        _ => "the selection".into(),
    }
}

/// The colour of a converted map's Halo 1 source drawn over it.
pub const OVERLAY_COLOR: [f32; 3] = [1.0, 0.55, 0.15];

/// The overlay's vertices: the Halo 1 source's triangles and glyphs, in
/// one colour (none when the overlay is off or has no source).
pub fn overlay_vertices(app: &App) -> Vec<render::Vertex> {
    let Some(v) = app.overlay() else {
        return Vec::new();
    };
    let mut out = render::overlay_vertices(&crate::app::library_tris(v), OVERLAY_COLOR);
    let mut glyphs = render::marker_vertices(&crate::app::library_markers(v), &[]);
    for g in &mut glyphs {
        g.color = OVERLAY_COLOR;
    }
    out.extend(glyphs);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::{RotationField, Transform};

    #[test]
    fn placing_snaps_x_and_y_to_the_grid_and_keeps_the_surface_height() {
        assert_eq!(snap_to_grid([1.26, -0.74, 3.33], 0.5), [1.5, -0.5, 3.33]);
        assert_eq!(snap_to_grid([1.26, -0.74, 3.33], 0.0), [1.26, -0.74, 3.33]);
    }

    #[test]
    fn the_gizmo_snaps_only_what_is_switched_on() {
        let mut s = Snap::default();
        assert_eq!(gizmo_snap(GizmoOp::Translate, &s), None);
        s.grid = true;
        assert_eq!(gizmo_snap(GizmoOp::Translate, &s), Some([0.5; 3]));
        assert_eq!(gizmo_snap(GizmoOp::Rotate, &s), None);
        s.angle = true;
        s.angle_step = 45.0;
        assert_eq!(gizmo_snap(GizmoOp::Rotate, &s), Some([45.0, 0.0, 0.0]));
        assert!(gizmo_snap(GizmoOp::Scale, &s).is_some());
    }

    #[test]
    fn each_placement_gets_the_gizmo_it_can_take() {
        let t = |rotation| Transform {
            position: [0.0; 3],
            rotation,
        };
        let ypr = t(Some(RotationField::Ypr([0.0; 3])));
        let facing = t(Some(RotationField::Facing(0.0)));
        let none = t(None);
        assert_eq!(
            gizmo_operation(GizmoOp::Rotate, &ypr, true),
            Operation::ROTATE
        );
        assert_eq!(
            gizmo_operation(GizmoOp::Rotate, &facing, false),
            Operation::ROTATE_Z
        );
        assert_eq!(
            gizmo_operation(GizmoOp::Rotate, &none, false),
            Operation::TRANSLATE
        );
        assert_eq!(
            gizmo_operation(GizmoOp::Scale, &ypr, true),
            Operation::SCALE_UNIFORM
        );
        // A spawn has no scale: R moves it.
        assert_eq!(
            gizmo_operation(GizmoOp::Scale, &facing, false),
            Operation::TRANSLATE
        );
    }
}
