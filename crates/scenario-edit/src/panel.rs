//! The egui panel: outliner, inspector and 3D viewport over one map.
//!
//! The shell owns the window; it calls [`ScenarioEditor::ui`] with its
//! `egui_wgpu::RenderState` every frame, and can drive saving and undo
//! through the public methods (menu items, shortcuts).
use crate::edit::{Command, History};
use crate::geometry::{self, Camera, Marker, Tri, Vec3};
use crate::layout::{BlockKind, FieldType, Role};
use crate::render::{self, Viewport};
use crate::save::{self, MapSource};
use crate::scenario::{Scenario, TagRef, Value};
use crate::{Error, Result};
use egui::{PointerButton, Sense};
use map::ResourceFile;
use std::path::{Path, PathBuf};

/// An open map.
struct Doc {
    source: MapSource,
    scenario: Scenario,
    history: History,
    /// Patches at the last save (or the source's, empty), for `is_dirty`.
    saved: Vec<(u32, Vec<u8>)>,
    world: Vec<Tri>,
}

/// A drag of the selected placement in the viewport.
#[derive(Clone, Copy, Debug)]
struct Drag {
    key: u64,
    /// Plane the pointer moves on, and the grab offset from the placement.
    plane_point: Vec3,
    plane_normal: Vec3,
    offset: Vec3,
    vertical: bool,
}

/// The scenario editor panel.
pub struct ScenarioEditor {
    doc: Option<Doc>,
    open_path: String,
    save_path: String,
    disc_maps: Vec<(String, MapSource)>,
    status: String,
    selected: Option<(BlockKind, usize)>,
    camera: Camera,
    viewport: Option<Viewport>,
    world_uploaded: bool,
    markers: Vec<Marker>,
    markers_dirty: bool,
    drag: Option<Drag>,
    next_key: u64,
    filter: String,
}

impl Default for ScenarioEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl ScenarioEditor {
    pub fn new() -> Self {
        Self {
            doc: None,
            open_path: String::new(),
            save_path: String::new(),
            disc_maps: Vec::new(),
            status: "Open a .map file, or a disc image to pick a map from it.".into(),
            selected: None,
            camera: Camera::default(),
            viewport: None,
            world_uploaded: false,
            markers: Vec::new(),
            markers_dirty: true,
            drag: None,
            next_key: 1,
            filter: String::new(),
        }
    }

    /// Open a map. The source is only read; edits stay in memory until
    /// [`Self::save_as`].
    pub fn open(&mut self, source: MapSource) -> Result<()> {
        let mut map = source.open()?;
        let scenario = Scenario::read(&mut map)?;
        let mut files = vec![(ResourceFile::Local, map)];
        for (file, s) in geometry::shared_sources(&source) {
            if let Ok(m) = s.open() {
                files.push((file, m));
            }
        }
        let mut raw = geometry::RawFiles::new(files);
        let mut map = source.open()?;
        let (world, world_failed) =
            geometry::world_triangles(&mut map, &mut raw).unwrap_or_default();
        let mut bounds = geometry::Bounds::EMPTY;
        for t in &world {
            t.p.iter().for_each(|&p| bounds.add(p));
        }
        let markers = geometry::markers(&scenario);
        if bounds.is_empty() {
            markers.iter().for_each(|m| bounds.add(m.position));
        }
        self.camera = Camera::frame(&bounds);
        self.status = format!(
            "{} triangles{}, {} placements",
            world.len(),
            if world_failed > 0 {
                format!(" ({world_failed} sections not loaded)")
            } else {
                String::new()
            },
            markers.len()
        );
        self.doc = Some(Doc {
            source,
            scenario,
            history: History::default(),
            saved: Vec::new(),
            world,
        });
        self.selected = None;
        self.world_uploaded = false;
        self.markers_dirty = true;
        self.drag = None;
        Ok(())
    }

    /// List the maps on a disc image, for the map picker.
    pub fn open_disc(&mut self, iso: &Path) -> Result<()> {
        self.disc_maps = MapSource::disc_maps(iso)?;
        self.status = format!("{} maps on the disc", self.disc_maps.len());
        Ok(())
    }

    pub fn scenario(&self) -> Option<&Scenario> {
        self.doc.as_ref().map(|d| &d.scenario)
    }
    pub fn selected(&self) -> Option<(BlockKind, usize)> {
        self.selected
    }
    pub fn select(&mut self, selection: Option<(BlockKind, usize)>) {
        if selection != self.selected
            && let Some(doc) = self.doc.as_mut()
        {
            doc.history.seal();
        }
        self.selected = selection;
        self.markers_dirty = true;
    }

    /// Edits since the last save (undoing back to the saved state clears it).
    pub fn is_dirty(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.scenario.patches() != d.saved)
    }

    /// Write the edited map to `path`: the source with the changed fields
    /// patched and the checksum repaired. Never overwrites the source.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<u64> {
        let doc = self.doc.as_mut().ok_or(Error::Save("no map is open"))?;
        let n = save::save_as(&doc.source, &doc.scenario, path.as_ref())?;
        doc.saved = doc.scenario.patches();
        doc.history.seal();
        self.status = format!("saved {} ({n} bytes)", path.as_ref().display());
        Ok(n)
    }

    /// Apply an edit as one undo step.
    pub fn apply(&mut self, command: Command) -> Result<()> {
        let doc = self.doc.as_mut().ok_or(Error::Save("no map is open"))?;
        doc.history.apply(&mut doc.scenario, command)?;
        self.markers_dirty = true;
        Ok(())
    }
    pub fn undo(&mut self) -> bool {
        self.step(true)
    }
    pub fn redo(&mut self) -> bool {
        self.step(false)
    }
    fn step(&mut self, undo: bool) -> bool {
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        self.drag = None;
        let r = if undo {
            doc.history.undo(&mut doc.scenario)
        } else {
            doc.history.redo(&mut doc.scenario)
        };
        self.markers_dirty = true;
        match r {
            Ok(Some(label)) => {
                self.status = format!("{} {label}", if undo { "undid" } else { "redid" });
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.status = e.to_string();
                false
            }
        }
    }
    pub fn can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.history.can_undo())
    }
    pub fn can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.history.can_redo())
    }

    /// Draw the panel.
    pub fn ui(&mut self, ui: &mut egui::Ui, render: &egui_wgpu::RenderState) {
        self.shortcuts(ui);
        egui::Panel::top("scenario_toolbar").show(ui, |ui| self.toolbar(ui));
        if self.doc.is_none() {
            ui.label(&self.status);
            return;
        }
        egui::Panel::left("scenario_outliner")
            .default_size(240.0)
            .show(ui, |ui| self.outliner(ui));
        egui::Panel::right("scenario_inspector")
            .default_size(280.0)
            .show(ui, |ui| self.inspector(ui));
        self.viewport(ui, render);
    }

    fn shortcuts(&mut self, ui: &egui::Ui) {
        let (undo, redo) = ui.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && (i.key_pressed(egui::Key::Y)
                    || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            )
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Map or disc:");
            ui.add(egui::TextEdit::singleline(&mut self.open_path).desired_width(260.0));
            if ui.button("Open").clicked() {
                let path = PathBuf::from(self.open_path.trim());
                let is_iso = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("iso"));
                let r = if is_iso {
                    self.open_disc(&path)
                } else {
                    self.open(MapSource::file(path))
                };
                if let Err(e) = r {
                    self.status = e.to_string();
                }
            }
            if !self.disc_maps.is_empty() {
                let mut chosen = None;
                egui::ComboBox::from_id_salt("scenario_disc_maps")
                    .selected_text("Disc map")
                    .show_ui(ui, |ui| {
                        for (name, s) in &self.disc_maps {
                            if ui.selectable_label(false, name).clicked() {
                                chosen = Some((name.clone(), s.clone()));
                            }
                        }
                    });
                if let Some((name, s)) = chosen {
                    if let Err(e) = self.open(s) {
                        self.status = e.to_string();
                    } else if self.save_path.is_empty() {
                        self.save_path = format!("{name}.map");
                    }
                }
            }
            ui.separator();
            ui.add_enabled_ui(self.can_undo(), |ui| {
                if ui.button("Undo").clicked() {
                    self.undo();
                }
            });
            ui.add_enabled_ui(self.can_redo(), |ui| {
                if ui.button("Redo").clicked() {
                    self.redo();
                }
            });
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.save_path).desired_width(200.0));
            let label = if self.is_dirty() {
                "Save as*"
            } else {
                "Save as"
            };
            if ui
                .add_enabled(self.doc.is_some(), egui::Button::new(label))
                .clicked()
            {
                let path = PathBuf::from(self.save_path.trim());
                if let Err(e) = self.save_as(&path) {
                    self.status = e.to_string();
                }
            }
        });
        ui.label(&self.status);
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter"));
        let Some(doc) = &self.doc else { return };
        let filter = self.filter.to_ascii_lowercase();
        let mut pick = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for b in &doc.scenario.blocks {
                let header = format!("{} ({})", b.def.label, b.len());
                egui::CollapsingHeader::new(header)
                    .id_salt(b.def.kind)
                    .show(ui, |ui| {
                        for i in 0..b.len() {
                            let label = doc.scenario.label(b.def.kind, i);
                            if !filter.is_empty() && !label.to_ascii_lowercase().contains(&filter) {
                                continue;
                            }
                            let on = self.selected == Some((b.def.kind, i));
                            if ui.selectable_label(on, label).clicked() {
                                pick = Some((b.def.kind, i));
                            }
                        }
                    });
            }
        });
        if let Some(p) = pick {
            self.select(Some(p));
            if let Some(m) = self.markers.iter().find(|m| (m.block, m.index) == p) {
                self.camera.target = m.position;
            }
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some((kind, index)) = self.selected else {
            ui.label("Select a placement in the outliner or the viewport.");
            return;
        };
        let Some(doc) = &self.doc else { return };
        let block = doc.scenario.block(kind);
        ui.heading(doc.scenario.label(kind, index));
        ui.label(format!(
            "{} #{index}, stride {:#x} ({})",
            block.def.label, block.def.stride, block.def.stride_evidence
        ));
        ui.separator();
        let mut edits = Vec::new();
        for f in block.def.fields {
            let Ok(value) = doc.scenario.get(kind, index, f.name) else {
                continue;
            };
            ui.horizontal(|ui| {
                ui.label(format!("{:#x} {}", f.offset, f.name))
                    .on_hover_text(f.evidence);
                ui.add_enabled_ui(f.editable, |ui| {
                    if let Some(v) = value_editor(ui, &doc.scenario, kind, f, value) {
                        edits.push((f.name, v));
                    }
                });
            });
        }
        if let Some(p) = &block.def.palette {
            ui.separator();
            ui.label(format!(
                "palette ({} entries, stride {:#x})",
                block.palette.len(),
                p.stride
            ));
        }
        for (field, v) in edits {
            // Typing into one field merges into one undo step until the
            // selection changes or the map is saved.
            let merge = hash_key(kind, index, field);
            if let Err(e) = self.apply(Command::set(kind, index, field, v).merging(merge)) {
                self.status = e.to_string();
            }
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, render: &egui_wgpu::RenderState) {
        let Some(doc) = &self.doc else { return };
        let viewport = self
            .viewport
            .get_or_insert_with(|| Viewport::new(&render.device));
        if !self.world_uploaded {
            viewport.set_world(&render.device, &doc.world);
            self.world_uploaded = true;
        }
        if self.markers_dirty {
            self.markers = geometry::markers(&doc.scenario);
            viewport.set_markers(
                &render.device,
                &render::marker_vertices(&self.markers, self.selected),
            );
            self.markers_dirty = false;
        }
        let avail = ui.available_size().max(egui::vec2(64.0, 64.0));
        let ppp = ui.ctx().pixels_per_point();
        let px = [(avail.x * ppp) as u32, (avail.y * ppp) as u32];
        let tex = viewport.egui_texture(render, &self.camera, px);
        let response = ui.add(
            egui::Image::new(egui::load::SizedTexture::new(tex, avail))
                .sense(Sense::click_and_drag()),
        );
        let rect = response.rect;
        let aspect = rect.width() / rect.height().max(1.0);
        let uv = |p: egui::Pos2| {
            (
                (p.x - rect.min.x) / rect.width(),
                (p.y - rect.min.y) / rect.height(),
            )
        };

        // Camera: right-drag orbits, middle-drag pans, scroll zooms, WASD/QE fly.
        if response.dragged_by(PointerButton::Secondary) {
            let d = response.drag_delta();
            self.camera.orbit(-d.x * 0.008, d.y * 0.008);
        }
        if response.dragged_by(PointerButton::Middle) {
            let d = response.drag_delta();
            self.camera.pan(d.x, d.y, rect.height());
        }
        if response.hovered() {
            let (scroll, keys, dt) = ui.input(|i| {
                let k = |key| if i.key_down(key) { 1.0 } else { 0.0 };
                (
                    i.smooth_scroll_delta.y,
                    [
                        k(egui::Key::D) - k(egui::Key::A),
                        k(egui::Key::E) - k(egui::Key::Q),
                        k(egui::Key::W) - k(egui::Key::S),
                    ],
                    i.stable_dt.min(0.1),
                )
            });
            if scroll != 0.0 {
                self.camera.zoom((-scroll * 0.002).exp());
            }
            if keys.iter().any(|&k| k != 0.0) {
                let speed = self.camera.distance.max(5.0) * dt;
                self.camera
                    .fly(keys[0] * speed, keys[1] * speed, keys[2] * speed);
                ui.ctx().request_repaint();
            }
        }

        // Selection and dragging with the primary button.
        if response.clicked_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let (u, v) = uv(p);
            let hit = geometry::pick(&self.markers, &self.camera.ray(u, v, aspect));
            self.select(hit);
        }
        if response.drag_started_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let (u, v) = uv(p);
            let ray = self.camera.ray(u, v, aspect);
            if let Some(hit) = geometry::pick(&self.markers, &ray) {
                self.select(Some(hit));
            }
            let vertical = ui.input(|i| i.modifiers.shift);
            self.drag = self.start_drag(&ray, vertical);
        }
        if response.dragged_by(PointerButton::Primary)
            && let (Some(drag), Some(p)) = (self.drag, response.interact_pointer_pos())
        {
            let (u, v) = uv(p);
            let ray = self.camera.ray(u, v, aspect);
            self.continue_drag(drag, &ray);
        }
        if response.drag_stopped()
            && self.drag.take().is_some()
            && let Some(doc) = self.doc.as_mut()
        {
            doc.history.seal();
        }
    }

    fn position_field(&self, kind: BlockKind) -> Option<&'static str> {
        crate::layout::block(kind)
            .role(Role::Position)
            .map(|f| f.name)
    }

    fn start_drag(&mut self, ray: &geometry::Ray, vertical: bool) -> Option<Drag> {
        let (kind, index) = self.selected?;
        let field = self.position_field(kind)?;
        let pos = self.scenario()?.get(kind, index, field).ok()?.as_vec3()?;
        let normal = if vertical {
            let f = self.camera.forward();
            geometry::normalize([f[0], f[1], 0.0])
        } else {
            [0.0, 0.0, 1.0]
        };
        let hit = ray.plane(pos, normal)?;
        let key = self.next_key;
        self.next_key += 1;
        Some(Drag {
            key,
            plane_point: pos,
            plane_normal: normal,
            offset: geometry::sub(pos, hit),
            vertical,
        })
    }

    fn continue_drag(&mut self, drag: Drag, ray: &geometry::Ray) {
        let Some((kind, index)) = self.selected else {
            return;
        };
        let Some(field) = self.position_field(kind) else {
            return;
        };
        let Some(hit) = ray.plane(drag.plane_point, drag.plane_normal) else {
            return;
        };
        let Some(Value::Vec3(old)) = self.scenario().and_then(|s| s.get(kind, index, field).ok())
        else {
            return;
        };
        let target = geometry::add(hit, drag.offset);
        let new = if drag.vertical {
            [old[0], old[1], target[2]]
        } else {
            [target[0], target[1], old[2]]
        };
        if new != old {
            let cmd = Command::set(kind, index, field, Value::Vec3(new)).merging(drag.key);
            if let Err(e) = self.apply(cmd) {
                self.status = e.to_string();
            }
        }
    }
}

fn hash_key(kind: BlockKind, index: usize, field: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (kind, index, field).hash(&mut h);
    h.finish()
}

/// A numeric editor for one value; returns the new value when changed.
fn value_editor(
    ui: &mut egui::Ui,
    scenario: &Scenario,
    kind: BlockKind,
    f: &crate::layout::FieldDef,
    value: Value,
) -> Option<Value> {
    match value {
        Value::Vec3(mut v) => {
            let speed = if f.role == Role::Rotation { 0.01 } else { 0.05 };
            let mut changed = false;
            for x in &mut v {
                changed |= ui.add(egui::DragValue::new(x).speed(speed)).changed();
            }
            changed.then_some(Value::Vec3(v))
        }
        Value::Float(mut x) => ui
            .add(egui::DragValue::new(&mut x).speed(0.01))
            .changed()
            .then_some(Value::Float(x)),
        Value::Int(mut x) => {
            if f.role == Role::PaletteIndex {
                let palette = &scenario.block(kind).palette;
                let name = |i: i64| -> String {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| palette.get(i))
                        .and_then(|r| scenario.tag_name(r.id))
                        .map(|n| format!("{i}: {n}"))
                        .unwrap_or_else(|| i.to_string())
                };
                let before = x;
                egui::ComboBox::from_id_salt(("palette", kind))
                    .selected_text(name(x))
                    .show_ui(ui, |ui| {
                        for i in -1..palette.len() as i64 {
                            ui.selectable_value(&mut x, i, name(i));
                        }
                    });
                return (x != before).then_some(Value::Int(x));
            }
            let range = match f.ty {
                FieldType::U8 => 0i64..=0xff,
                FieldType::I16 => -0x8000..=0x7fff,
                FieldType::U16 => 0..=0xffff,
                _ => 0..=0xffff_ffff,
            };
            ui.add(egui::DragValue::new(&mut x).range(range).speed(0.1))
                .changed()
                .then_some(Value::Int(x))
        }
        Value::Tag(r) => {
            let mut chosen = r;
            let text = |r: TagRef| {
                if r.is_none() {
                    "none".to_owned()
                } else {
                    format!("{}: {}", r.group, scenario.tag_name(r.id).unwrap_or("?"))
                }
            };
            egui::ComboBox::from_id_salt(("tag", kind, f.offset))
                .selected_text(text(r))
                .show_ui(ui, |ui| {
                    let mut options: Vec<TagRef> = scenario
                        .tags
                        .iter()
                        .filter(|(_, t)| crate::layout::ITEM_GROUPS.contains(&t.group.0))
                        .map(|(&id, t)| TagRef { group: t.group, id })
                        .collect();
                    options.sort_by_key(|o| text(*o));
                    for o in options {
                        ui.selectable_value(&mut chosen, o, text(o));
                    }
                });
            (chosen != r).then_some(Value::Tag(chosen))
        }
    }
}
