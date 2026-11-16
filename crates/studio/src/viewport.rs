//! The 3D viewport: the scenario crate's offscreen wgpu renderer
//! ([`scenario_edit::render::Viewport`]: the textured structure BSP, the
//! placed objects' models and a glyph per placement without one) registered with `dear-imgui-wgpu` as an external texture and
//! drawn as an ImGui image, with picking, an orbit/fly camera and an
//! ImGuizmo translate/rotate gizmo on the selection.
use dear_imgui_rs::{MouseButton, TextureId, Ui};
use dear_imgui_wgpu::{ExternalTextureId, WgpuRenderer};
use dear_imguizmo::{GuizmoExt, Mode, Operation};
use scenario_edit::geometry;
use scenario_edit::render::{self, Viewport};

use crate::app::{Action, App};

/// The gizmo's operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoOp {
    Translate,
    Rotate,
}

/// What the viewport needs from the GPU side while the UI is built.
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
    pub op: GizmoOp,
    /// Gizmo axes follow the placement (local) or the world.
    pub local: bool,
    /// Right mouse is held over the viewport: orbit and WASD fly.
    looking: bool,
    panning: bool,
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
            op: GizmoOp::Translate,
            local: false,
            looking: false,
            panning: false,
        }
    }

    /// Whether the camera has the mouse (keyboard shortcuts then fly).
    pub fn looking(&self) -> bool {
        self.looking
    }

    /// Render the scene at `size_px` and return the ImGui texture showing it.
    fn prepare(&mut self, gpu: &mut Gpu<'_>, app: &App, size_px: [u32; 2]) -> Option<TextureId> {
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
        if world_changed || self.markers_rev != Some(session.markers_rev()) {
            match session.scene() {
                Some(scene) => {
                    vp.set_placements(gpu.device, scene, session.markers(), session.selected())
                }
                None => vp.set_markers(
                    gpu.device,
                    &render::marker_vertices(session.markers(), session.selected()),
                ),
            }
            self.markers_rev = Some(session.markers_rev());
        }
        let resized = vp.render(gpu.device, gpu.queue, session.camera(), size_px);
        let view = vp.color_view()?;
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
        let Some(texture) = self.prepare(gpu, app, px) else {
            ui.text_disabled("The viewport could not create its render target.");
            return;
        };
        ui.image(texture, size);
        let hovered = ui.is_item_hovered();
        let aspect = size[0] / size[1].max(1.0);

        // The gizmo on the selection.
        let camera = *app.session.camera();
        let view = geometry::flat(&camera.view());
        let proj = geometry::flat(&camera.proj(aspect));
        let mut over_gizmo = false;
        let mut using_gizmo = false;
        if let Some(t) = app.session.selected_transform() {
            let mut model = t.matrix();
            let gizmo = ui.guizmo();
            gizmo.set_drawlist_window();
            gizmo.set_rect(origin[0], origin[1], size[0], size[1]);
            gizmo.set_orthographic(false);
            let op = match (self.op, t.rotation) {
                (GizmoOp::Translate, _) => Operation::TRANSLATE,
                // Facing-only placements turn about z alone.
                (GizmoOp::Rotate, Some(scenario_edit::RotationField::Facing(_))) => {
                    Operation::ROTATE_Z
                }
                (GizmoOp::Rotate, Some(_)) => Operation::ROTATE,
                (GizmoOp::Rotate, None) => Operation::TRANSLATE,
            };
            let mode = if self.local { Mode::Local } else { Mode::World };
            let changed = gizmo
                .manipulate_config(&view, &proj, &mut model)
                .operation(op)
                .mode(mode)
                .build();
            over_gizmo = gizmo.is_over();
            using_gizmo = gizmo.is_using();
            if changed {
                let (pos, ypr) = geometry::decompose(&model);
                let ypr = match t.rotation {
                    // Translating never touches the rotation.
                    _ if self.op == GizmoOp::Translate => t.ypr(),
                    Some(_) => ypr,
                    None => [0.0; 3],
                };
                let pos = if self.op == GizmoOp::Rotate {
                    t.position
                } else {
                    pos
                };
                if let Err(e) = app.session.gesture_set(pos, ypr) {
                    app.error(e.to_string());
                }
            }
        }
        if !using_gizmo && app.session.in_gesture() {
            app.session.end_gesture();
            let what = selection_label(app);
            app.info(format!("moved {what}"));
        }

        // Camera: right-drag orbits (with WASD/QE flying while held),
        // middle-drag pans, the wheel zooms.
        let io = ui.io();
        if hovered && ui.is_mouse_clicked(MouseButton::Right) {
            self.looking = true;
        }
        if !ui.is_mouse_down(MouseButton::Right) {
            self.looking = false;
        }
        if hovered && ui.is_mouse_clicked(MouseButton::Middle) {
            self.panning = true;
        }
        if !ui.is_mouse_down(MouseButton::Middle) {
            self.panning = false;
        }
        let d = io.mouse_delta();
        let cam = app.session.camera_mut();
        if self.looking {
            cam.orbit(-d[0] * 0.008, d[1] * 0.008);
            let k = |key| if ui.is_key_down(key) { 1.0 } else { 0.0 };
            use dear_imgui_rs::Key;
            let keys = [
                k(Key::D) - k(Key::A),
                k(Key::E) - k(Key::Q),
                k(Key::W) - k(Key::S),
            ];
            if keys.iter().any(|&k| k != 0.0) {
                let speed = cam.distance.max(5.0) * io.delta_time().min(0.1);
                cam.fly(keys[0] * speed, keys[1] * speed, keys[2] * speed);
            }
        }
        if self.panning {
            cam.pan(d[0], d[1], size[1]);
        }
        if hovered && io.mouse_wheel() != 0.0 {
            cam.zoom((-io.mouse_wheel() * 0.12).exp());
        }

        // Left click (not on the gizmo) picks.
        if hovered && !over_gizmo && !using_gizmo && ui.is_mouse_clicked(MouseButton::Left) {
            let m = io.mouse_pos();
            let u = (m[0] - origin[0]) / size[0];
            let v = (m[1] - origin[1]) / size[1];
            let ray = app.session.camera().ray(u, v, aspect);
            actions.push(Action::Select(app.session.pick(&ray)));
        }
    }
}

fn selection_label(app: &App) -> String {
    match (app.session.selected(), app.session.scenario()) {
        (Some((kind, i)), Some(s)) => format!("{:?} {}", kind, s.label(kind, i)),
        _ => "the selection".into(),
    }
}
