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
use scenario_edit::layers::{LayerId, Layers};
use scenario_edit::render::{self, Viewport};
use scenario_edit::volumes::{Frame, VolumeKind};
use scenario_edit::{BlockKind, Session, geometry};

use crate::app::{Action, App};
use crate::camera;
use crate::play::select;

/// The gizmo's operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoOp {
    Translate,
    Rotate,
    /// Uniform scale, for placements with a scale (objects); a trigger
    /// volume's box scales along each of its axes.
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

/// The gizmo on a trigger volume: it moves, turns and stretches the box
/// along each axis.
pub fn volume_gizmo_operation(op: GizmoOp) -> Operation {
    match op {
        GizmoOp::Translate => Operation::TRANSLATE,
        GizmoOp::Rotate => Operation::ROTATE,
        GizmoOp::Scale => Operation::SCALE,
    }
}

/// The box the gizmo's matrix gives a volume that was `before`: a move
/// changes only the centre, a turn only the axes (about the centre), a
/// scale only the size (about the centre).
pub fn volume_gizmo_frame(op: GizmoOp, before: &Frame, model: &[f32; 16]) -> Option<Frame> {
    let moved = Frame::from_matrix(model)?;
    let (centre, [forward, _, up]) = (before.center()?, before.axes()?);
    match op {
        GizmoOp::Translate => Frame::around(moved.center()?, forward, up, before.size),
        GizmoOp::Rotate => Frame::around(centre, moved.forward, moved.up, before.size),
        GizmoOp::Scale => Frame::around(centre, forward, up, moved.size),
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
    /// The session's native models last uploaded: a placement of a tag
    /// first placed after the map opened gives the session new ones.
    /// The geometry session revision the native world last redrew.
    modeling_rev: Option<u64>,
    native_models: Option<std::sync::Arc<scenario_edit::native_view::NativeModels>>,
    linked: BTreeSet<(BlockKind, usize)>,
    overlay_rev: Option<u64>,
    layers: Option<Layers>,
    labels: Vec<mirror::Label>,
    live_scene: mirror::SceneCache,
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
    hovered: bool,
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
            modeling_rev: None,
            native_models: None,
            linked: BTreeSet::new(),
            overlay_rev: None,
            layers: None,
            labels: Vec::new(),
            live_scene: Default::default(),
            op: GizmoOp::Translate,
            local: false,
            snap: Snap::default(),
            camera: camera::Controller::new(),
            camera_world: None,
            flying: false,
            orbiting: false,
            panning: false,
            placing: None,
            hovered: false,
            shown: None,
        }
    }

    /// Where the last frame showed the viewport (origin and size in
    /// points), and its render target: the headless tests compare the two.
    pub fn shown(&self) -> Option<([f32; 4], &wgpu::Texture)> {
        Some((self.shown?, self.vp.as_ref()?.color_texture()?))
    }

    pub fn hovered(&self) -> bool {
        self.hovered
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
    fn prepare(
        &mut self,
        gpu: &mut Gpu<'_>,
        app: &App,
        live: &mirror::Scene,
        live_changed: bool,
        size_px: [u32; 2],
    ) -> Option<TextureId> {
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
            // The level itself through the native renderer, when its BSPs
            // loaded (docs/editor/polish-plan.md, P14).
            let native = session.native_world().map_or(&[][..], |n| n.as_slice());
            vp.set_native(gpu.device, gpu.queue, native);
            self.world_rev = Some(session.world_rev());
        }
        // The Modeling tool's in-place edits, redrawn over the sections
        // they came from (docs/editor/modeling.md, "Interfaces").
        if world_changed || self.modeling_rev != Some(app.modeling_rev()) {
            for p in app.modeling_previews() {
                for d in &p.drawn {
                    let owner = match *d {
                        geometry_edit::Drawn::Cluster(i) => render::SectionOwner::Cluster(i),
                        geometry_edit::Drawn::Instance(i) => render::SectionOwner::Instance(i),
                    };
                    vp.set_native_section(gpu.queue, p.bsp, owner, &p.positions, None);
                }
            }
            self.modeling_rev = Some(app.modeling_rev());
        }
        if vp.lighting() != app.lighting {
            let native = session.native_world().map_or(&[][..], |n| n.as_slice());
            vp.set_lighting(gpu.device, gpu.queue, native, app.lighting);
        }
        // Its placed objects too, again whenever the session reads a new
        // tag's model; the scene draws only those with no native model.
        let models = session.native_models();
        let models_changed = world_changed
            || match (&self.native_models, models) {
                (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
                (None, None) => false,
                _ => true,
            };
        if models_changed {
            let upload = models.cloned().unwrap_or_default();
            vp.set_native_models(gpu.device, gpu.queue, upload);
            self.native_models = models.cloned();
        }
        let layers_changed = self.layers.as_ref() != Some(&app.layers);
        if layers_changed {
            vp.set_layers(&app.layers);
            self.layers = Some(app.layers.clone());
        }
        if world_changed || models_changed || layers_changed
            || self.markers_rev != Some(session.markers_rev())
            || self.linked != live.linked
        {
            upload_placements(vp, gpu.device, app, &live.linked);
            self.labels = mirror::gameplay_labels(app);
            self.markers_rev = Some(session.markers_rev());
            self.linked.clone_from(&live.linked);
        }
        if world_changed || live_changed {
            vp.set_live_instances(gpu.device, gpu.queue, session.source(), &live.instances);
        }
        if self.overlay_rev != Some(app.overlay_rev()) {
            upload_overlays(vp, gpu.device, app);
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
        self.hovered = false;
        let (live, live_changed) = self.live_scene.shown(app);
        let Some(texture) = self.prepare(gpu, app, &live, live_changed, px) else {
            ui.text_disabled("The viewport could not create its render target.");
            return;
        };
        ui.image(texture, size);
        let rect = [origin[0], origin[1], size[0], size[1]];
        self.shown = Some(rect);
        mirror::draw_labels(ui, app.session.camera(), rect, &self.labels);
        live.overlay(ui, app.session.camera(), rect);
        let hovered = ui.is_item_hovered();
        self.hovered = hovered;
        let aspect = size[0] / size[1].max(1.0);

        let (over_gizmo, using_gizmo) = self.gizmo(ui, app, origin, size, aspect, &live.linked);

        let io = ui.io();
        let alt = io.key_alt();
        let cmd = io.key_ctrl() || io.key_super();
        let m = io.mouse_pos();
        let uv = [(m[0] - origin[0]) / size[0], (m[1] - origin[1]) / size[1]];
        let ray = app.session.camera().ray(uv[0], uv[1], aspect);
        app.pairing.camera.cursor = hovered.then_some((ray, app.session.world_rev()));

        // Camera: right button looks and flies, Alt + left orbits, middle
        // pans, the wheel dollies (or sets the fly speed while flying).
        if hovered && ui.is_mouse_clicked(MouseButton::Right) {
            self.flying = true;
        }
        if !ui.is_mouse_down(MouseButton::Right) {
            self.flying = false;
        }
        if hovered && alt && !using_gizmo && ui.is_mouse_clicked(MouseButton::Left) {
            let pivot = app
                .session
                .selection_sphere()
                .map(|(c, _)| c)
                .or_else(|| app.session.surface_hit_visible(&ray, &app.layers))
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
        self.camera.speed = app.fly_speed();
        self.camera
            .update_shared(app.session.camera_mut(), &input, app.pairing.camera.mode);
        app.set_fly_speed(self.camera.speed);

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
            match app.session.surface_hit_visible(&ray, &app.layers) {
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
        let double =
            hovered && !alt && !over_gizmo && ui.is_mouse_double_clicked(MouseButton::Left);
        if (clicked || double)
            && let Some(point) = select_live_at(app, &ray, &live.linked, cmd && !double, actions)
        {
            if double {
                let to = camera::focus_point(app.session.camera(), point, 30.0);
                self.camera.go_to(app.session.camera_mut(), to);
            }
            return;
        }
        // Double-click focuses what is under the cursor: a placement, else
        // the surface.
        if double {
            match app.session.pick_visible_linked(&ray, &app.layers, &live.linked) {
                Some(hit) => {
                    actions.push(Action::Select(Some(hit)));
                    actions.push(Action::FocusSelected);
                }
                None => {
                    if let Some(volume) = app.session.pick_volume(&ray, &app.layers) {
                        actions.push(Action::SelectVolume(Some(volume)));
                        actions.push(Action::FocusSelected);
                    } else if let Some(surface) = app.session.pick_surface(&ray, &app.layers) {
                        actions.push(Action::SelectSurface(Some(surface)));
                        actions.push(Action::FocusSelected);
                    } else if let Some(p) = app.session.surface_hit_visible(&ray, &app.layers) {
                        let to = camera::focus_point(app.session.camera(), p, 30.0);
                        self.camera.go_to(app.session.camera_mut(), to);
                    }
                }
            }
            return;
        }
        // Left click picks; with Ctrl/Cmd it adds to or takes from the
        // selection.
        if clicked && let Some(action) = pick_action(&app.session, &ray, cmd, &app.layers, &live.linked) {
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
        linked: &BTreeSet<(BlockKind, usize)>,
    ) -> (bool, bool) {
        let camera = *app.session.camera();
        let view = geometry::flat(&camera.view());
        let proj = geometry::flat(&camera.proj(aspect));
        let mut over = false;
        let mut using = false;
        if let Some(r) = self.volume_gizmo(ui, app, origin, size, &view, &proj) {
            (over, using) = r;
        }
        let visible = app.session.selected().is_some_and(|(kind, i)| {
            app.layers.placement(kind) && (app.layers.visible(LayerId::Markers)
                || (app.layers.visible(LayerId::Objects) && !linked.contains(&(kind, i))
                    && app.session.markers().iter().find(|m| (m.block, m.index) == (kind, i))
                        .is_some_and(|m| app.session.scene().and_then(|s| s.model_of(m)).is_some())))
        });
        if visible && let Some(t) = app.session.selected_transform() {
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

    /// The gizmo on the selected world-space trigger volume, when one is
    /// selected and shown: whether the mouse is over it and whether it is
    /// being dragged.
    fn volume_gizmo(
        &mut self,
        ui: &Ui,
        app: &mut App,
        origin: [f32; 2],
        size: [f32; 2],
        view: &[f32; 16],
        proj: &[f32; 16],
    ) -> Option<(bool, bool)> {
        let index = app.session.selected_volume()?;
        let before = app.session.selected_volume_frame()?;
        let shown = app
            .session
            .volumes()?
            .kinds(index)
            .into_iter()
            .any(|k| app.layers.visible(LayerId::Volume(k)));
        if !shown {
            return None;
        }
        let mut model = before.matrix()?;
        let gizmo = ui.guizmo();
        gizmo.set_drawlist_window();
        gizmo.set_rect(origin[0], origin[1], size[0], size[1]);
        gizmo.set_orthographic(false);
        let mode = if self.local { Mode::Local } else { Mode::World };
        let mut config = gizmo
            .manipulate_config(view, proj, &mut model)
            .operation(volume_gizmo_operation(self.op))
            .mode(mode);
        if let Some(snap) = gizmo_snap(self.op, &self.snap) {
            config = config.snap(snap);
        }
        let changed = config.build();
        let r = (gizmo.is_over(), gizmo.is_using());
        if changed && let Some(frame) = volume_gizmo_frame(self.op, &before, &model) {
            // Through the command layer, as the agent API's edits.
            app.dispatch(Action::VolumeGesture(frame));
        }
        Some(r)
    }
}

/// A linked hit uses the same frame action as an authored placement click.
fn select_live_at(
    app: &mut App,
    ray: &geometry::Ray,
    linked: &BTreeSet<(BlockKind, usize)>,
    extend: bool,
    actions: &mut Vec<Action>,
) -> Option<geometry::Vec3> {
    let hit = clicked_live(app, ray, linked)?;
    if let Some(action) = select::placement_action(app, hit, extend) {
        actions.push(action);
    } else {
        select::apply_loose(app, hit);
    }
    Some(hit.point)
}

/// A foreground authoring glyph or static mesh can cover a live object.
fn clicked_live(
    app: &App,
    ray: &geometry::Ray,
    linked: &BTreeSet<(BlockKind, usize)>,
) -> Option<select::Hit> {
    let live = select::viewport_pick(app, ray)?;
    let session = &app.session;
    let Some(entry) = session.pick_visible_linked(ray, &app.layers, linked) else {
        return Some(live);
    };
    let Some(marker) = session
        .markers()
        .iter()
        .find(|m| (m.block, m.index) == entry)
    else {
        return Some(live);
    };
    let model = session
        .scene()
        .and_then(|s| s.models.get(s.model_of(marker)?));
    let glyph_drawn = app.layers.visible(LayerId::Markers)
        && (model.is_none() || linked.contains(&entry) || session.selection().contains(&entry));
    if glyph_drawn && geometry::pick_glyph([marker], ray).is_some_and(|(t, _, _)| t < live.distance)
    {
        return None;
    }
    if app.layers.visible(LayerId::Objects)
        && !linked.contains(&entry)
        && let Some(model) = model
    {
        let scale = marker.scale.max(1e-6);
        let project = |v| marker.axes.map(|a| geometry::dot(v, a));
        let local = geometry::Ray {
            origin: geometry::scale(
                project(geometry::sub(ray.origin, marker.position)),
                scale.recip(),
            ),
            dir: project(ray.dir),
        };
        if model.hit(&local).is_some_and(|t| t * scale < live.distance) {
            return None;
        }
    }
    Some(live)
}

pub(crate) fn pick_action(
    session: &Session,
    ray: &geometry::Ray,
    extend: bool,
    layers: &Layers,
    linked: &BTreeSet<(BlockKind, usize)>,
) -> Option<Action> {
    let hit = session.pick_visible_linked(ray, layers, linked);
    if extend {
        return hit.map(Action::ToggleSelect);
    }
    // Placements sit inside volumes, so a placement under the cursor wins.
    if hit.is_none()
        && let Some(volume) = session.pick_volume(ray, layers)
    {
        return Some(Action::SelectVolume(Some(volume)));
    }
    // A shown collision surface is picked when nothing else is.
    if hit.is_none()
        && let Some(surface) = session.pick_surface(ray, layers)
    {
        return Some(Action::SelectSurface(Some(surface)));
    }
    Some(Action::Select(hit))
}

fn selection_label(app: &App) -> String {
    let n = app.session.selection().len();
    if let (Some(i), Some(v)) = (app.session.selected_volume(), app.session.volumes()) {
        return v.label(i);
    }
    if let Some(s) = app.session.selected_surface_data() {
        return surface_label(s);
    }
    match (app.session.selected(), app.session.scenario()) {
        _ if n > 1 => format!("{n} placements"),
        (Some((kind, i)), Some(s)) => format!("{:?} {}", kind, s.label(kind, i)),
        _ => "the selection".into(),
    }
}

/// Both the interactive view and view.snapshot use the same placement filters.
pub(crate) fn upload_placements(
    vp: &mut Viewport,
    device: &wgpu::Device,
    app: &App,
    linked: &BTreeSet<(BlockKind, usize)>,
) {
    let session = &app.session;
    let markers: Vec<_> = session.markers().iter().copied()
        .filter(|m| app.layers.placement(m.block))
        .collect();
    let selected = session.selection();
    let gameplay = |m: &geometry::Marker| geometry::Gameplay::of(session.scenario(), m);
    let style = |m: &geometry::Marker| {
        let data = gameplay(m);
        (crate::theme::placement_color(m.block, data.team), data.flag_type)
    };
    match session.scene() {
        Some(scene) => vp.set_linked_placements_styled(
            device, scene, &markers, &selected, linked, style,
        ),
        None => vp.set_markers(
            device, &render::glyph_vertices_styled(&markers, &selected, |_| false, style),
        ),
    }

    let mut lines = Vec::new();
    if app.layers.visible(LayerId::Markers) {
        let color = crate::theme::layer_color(LayerId::Flags);
        for outline in geometry::flag_outlines(&markers, gameplay) {
            let mut segment = |a, b| {
                for position in [a, b] {
                    lines.push(render::Vertex {
                        position,
                        center: position,
                        normal: [0.0, 0.0, 1.0],
                        color,
                    });
                }
            };
            for edge in outline.windows(2) {
                segment(edge[0], edge[1]);
            }
            if outline.len() > 2 {
                segment(outline[outline.len() - 1], outline[0]);
            }
        }
    }
    vp.set_layer_overlay(device, LayerId::Flags, &[], &lines);
    upload_volumes(vp, device, app);
    upload_collision(vp, device, app);
}

/// A collision surface as the status line and inspector name it.
pub(crate) fn surface_label(s: &scenario_edit::collision_view::CollisionSurface) -> String {
    match s.instance {
        Some(i) => format!("BSP {} instance {i} collision surface {}", s.bsp, s.surface),
        None => format!("BSP {} collision surface {}", s.bsp, s.surface),
    }
}

/// The level collision as three overlays: the structure's surfaces under
/// Level collision, instanced geometry's under Instance collision, the
/// uncovered ones of either again under Collision only (in its own
/// colour), each as translucent faces and wire edges, the selected
/// surface's edges in the selection colour; and the placed objects'
/// collision shapes as wire under Object collision. A hidden layer uploads
/// nothing, so a large map's collision costs nothing until it is shown.
fn upload_collision(vp: &mut Viewport, device: &wgpu::Device, app: &App) {
    use scenario_edit::collision_view::{CollisionSurface, SurfaceClass};
    let selected = app.session.selected_surface();
    let layers: [(LayerId, fn(&CollisionSurface) -> bool); 3] = [
        (LayerId::LevelCollision, |s| s.instance.is_none()),
        (LayerId::InstanceCollision, |s| s.instance.is_some()),
        (LayerId::CollisionOnly, |s| {
            s.class == SurfaceClass::CollisionOnly
        }),
    ];
    for (id, keep) in layers {
        let (mut faces, mut lines) = (Vec::new(), Vec::new());
        if let Some(c) = app.session.collision().filter(|_| app.layers.visible(id)) {
            let color = crate::theme::layer_color(id);
            for (i, s) in c.surfaces.iter().enumerate() {
                if !keep(s) {
                    continue;
                }
                let n = s.normal();
                let tris: Vec<geometry::Tri> = s
                    .triangles()
                    .into_iter()
                    .map(|p| geometry::Tri { p, n })
                    .collect();
                faces.extend(render::overlay_vertices(&tris, color));
                let edge = if selected == Some(i) {
                    crate::theme::VOLUME_SELECTED
                } else {
                    color
                };
                for position in s.edges().into_iter().flatten() {
                    lines.push(render::Vertex {
                        position,
                        center: position,
                        normal: n,
                        color: edge,
                    });
                }
            }
        }
        vp.set_layer_overlay(device, id, &faces, &lines);
    }
    let mut lines = Vec::new();
    if app.layers.visible(LayerId::ObjectCollision) {
        let color = crate::theme::layer_color(LayerId::ObjectCollision);
        let selection = app.session.selection();
        for shape in app.session.placed_object_shapes() {
            let m = shape.marker;
            if !app.layers.placement(m.block) {
                continue;
            }
            let edge = if selection.contains(&(m.block, m.index)) {
                crate::theme::VOLUME_SELECTED
            } else {
                color
            };
            for position in shape.lines.into_iter().flatten() {
                lines.push(render::Vertex {
                    position,
                    center: position,
                    normal: [0.0, 0.0, 1.0],
                    color: edge,
                });
            }
        }
    }
    vp.set_layer_overlay(device, LayerId::ObjectCollision, &[], &lines);
}

/// The trigger volumes as boxes, one overlay per kind a box is drawn as
/// (translucent faces and wire edges), the selected box's edges in the
/// selection colour. Parented volumes are left out: their frame is
/// relative to an object only a running game places.
fn upload_volumes(vp: &mut Viewport, device: &wgpu::Device, app: &App) {
    let empty = scenario_edit::volumes::Volumes::default();
    let volumes = app.session.volumes().unwrap_or(&empty);
    let selected = app.session.selected_volume();
    for kind in VolumeKind::ALL {
        let (mut faces, mut lines) = (Vec::new(), Vec::new());
        let color = crate::theme::volume_color(kind);
        for (i, volume) in volumes.triggers.iter().enumerate() {
            if !volume.in_world() || !volumes.kinds(i).contains(&kind) {
                continue;
            }
            let (Some(tris), Some(edges)) = (volume.frame.faces(), volume.frame.edges()) else {
                continue;
            };
            faces.extend(render::overlay_vertices(&tris, color));
            let edge = if selected == Some(i) {
                crate::theme::VOLUME_SELECTED
            } else {
                color
            };
            for position in edges.into_iter().flatten() {
                lines.push(render::Vertex {
                    position,
                    center: position,
                    normal: [0.0, 0.0, 1.0],
                    color: edge,
                });
            }
        }
        vp.set_layer_overlay(device, LayerId::Volume(kind), &faces, &lines);
    }
}

pub(crate) fn upload_overlays(vp: &mut Viewport, device: &wgpu::Device, app: &App) {
    let mut tris = Vec::new();
    if let Some(source) = app.compare().and_then(|c| c.view.as_ref().ok()) {
        let color = crate::theme::layer_color(LayerId::Halo1);
        tris = render::overlay_vertices(&crate::app::library_tris(source), color);
        tris.extend(render::glyph_vertices_styled(
            &crate::app::library_markers(source),
            &[],
            |_| false,
            |_| (color, None),
        ));
    }
    vp.set_layer_overlay(device, LayerId::Halo1, &tris, &[]);
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

    /// A volume's gizmo matrix is its box: a move keeps the axes and size,
    /// a turn keeps the centre and size, a stretch keeps the centre and
    /// axes, whatever else the matrix drifted by.
    #[test]
    fn a_volume_gizmo_moves_turns_or_stretches_only() {
        let before = Frame::around(
            [1.0, 2.0, 3.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [4.0, 2.0, 1.0],
        )
        .unwrap();
        let m = before.matrix().unwrap();
        assert_eq!(m[12..15], [1.0, 2.0, 3.0]);
        assert_eq!(Frame::from_matrix(&m), Some(before));
        assert_eq!(volume_gizmo_operation(GizmoOp::Scale), Operation::SCALE);

        // Moved by (5, 0, 0) and, by float drift, stretched a hair.
        let mut moved = m;
        moved[12] += 5.0;
        moved[0] *= 1.001;
        let f = volume_gizmo_frame(GizmoOp::Translate, &before, &moved).unwrap();
        let c = f.center().unwrap();
        assert!(
            (0..3).all(|k| (c[k] - [6.0, 2.0, 3.0][k]).abs() < 1e-5),
            "{c:?}"
        );
        assert_eq!(f.size, before.size);
        assert_eq!(f.forward, before.forward);

        // Turned a quarter about z: forward becomes +y; the centre stays.
        let mut turned = m;
        turned[0..3].copy_from_slice(&[0.0, 4.0, 0.0]);
        turned[4..7].copy_from_slice(&[-2.0, 0.0, 0.0]);
        turned[12] += 0.5;
        let f = volume_gizmo_frame(GizmoOp::Rotate, &before, &turned).unwrap();
        assert_eq!(f.forward, [0.0, 1.0, 0.0]);
        assert_eq!(f.center(), Some([1.0, 2.0, 3.0]));
        assert_eq!(f.size, before.size);

        // Stretched to twice the height.
        let mut tall = m;
        tall[10] = 2.0;
        let f = volume_gizmo_frame(GizmoOp::Scale, &before, &tall).unwrap();
        assert_eq!(f.size, [4.0, 2.0, 2.0]);
        assert_eq!(f.center(), Some([1.0, 2.0, 3.0]));
        assert_eq!(f.forward, before.forward);
    }
}

#[cfg(test)]
mod live_click_tests {
    use super::*;
    use crate::play::testing::Temp;
    use agent_proto::live::Object;

    #[test]
    fn click_on_moved_live_object_selects_its_authored_placement() {
        let temp = Temp::new("viewport-live-click");
        let mut app = App::new();
        app.set_data_root(temp.0.join("data"));
        app.dispatch(Action::OpenSource(temp.map("example")));
        let world = app.session.world_rev();
        let id = 0x0200_0003;
        app.pairing.stream_map = serde_json::json!({
            "epoch": 1, "name": "example", "checksum": app.session.source_checksum()
        });
        app.pairing.shown = Some(live_mirror::Sample {
            tick: 1,
            epoch: 1,
            stale: false,
            cameras: Vec::new(),
            players: vec![],
            objects: vec![Object {
                id,
                tag: Some(scenario_edit::fixture::id(2).0),
                kind: 2,
                placement: Some(0),
                position: [92.0, 12.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                scale: 1.0,
                ..Default::default()
            }],
        });
        app.layers.set(LayerId::Live, true);
        let ray = geometry::Ray {
            origin: [90.0, 12.0, 0.0],
            dir: [1.0, 0.0, 0.0],
        };
        let live = mirror::Scene::shown(&app);
        let mut actions = Vec::new();
        app.dispatch(Action::SetTool(crate::app::ToolKind::Tags));
        assert!(select_live_at(&mut app, &ray, &live.linked, false, &mut actions).is_some());
        assert_eq!(actions, [Action::Select(Some((BlockKind::Weapons, 0)))]);
        assert_eq!(app.tool, crate::app::ToolKind::Tags);
        assert_eq!(app.session.selected(), None);
        crate::ui::dispatch_actions(&mut app, actions);
        assert_eq!(app.tool, crate::app::ToolKind::Scenario);
        assert_eq!(app.session.selected(), Some((BlockKind::Weapons, 0)));
        assert_eq!(app.session.world_rev(), world);

        // Extending selection goes through the same queue and inspector switch.
        app.dispatch(Action::SetTool(crate::app::ToolKind::Tags));
        let mut actions = Vec::new();
        assert!(select_live_at(&mut app, &ray, &live.linked, true, &mut actions).is_some());
        assert_eq!(actions, [Action::ToggleSelect((BlockKind::Weapons, 0))]);
        crate::ui::dispatch_actions(&mut app, actions);
        assert_eq!(app.tool, crate::app::ToolKind::Scenario);
        assert_eq!(app.session.selected(), None);

        // Removing entry 0 moves the old entry 1 into that slot.
        app.session.remove_entry(BlockKind::Weapons, 0).unwrap();
        let live = mirror::Scene::shown(&app);
        assert!(live.linked.is_empty());
        let hit = clicked_live(&app, &ray, &live.linked).unwrap();
        assert_eq!(hit.placement, None);
        select::apply(&mut app, hit, false);
        assert_eq!(select::selected_object(&app), Some(id));
        assert_eq!(app.selection(), Some(crate::app::Selection::Tag(scenario_edit::fixture::id(2))));
        // Choosing this same definition in the tag tree selects all instances of it.
        app.dispatch(Action::Navigate(crate::app::NavAction::SelectTag(
            scenario_edit::fixture::id(2),
        )));
        assert_eq!(select::selected_object(&app), None);
    }
}
