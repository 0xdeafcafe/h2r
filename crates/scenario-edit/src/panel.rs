//! The egui panel: outliner, inspector and 3D viewport over one map.
//!
//! The shell owns the window; it calls [`ScenarioEditor::ui`] with its
//! `egui_wgpu::RenderState` every frame, and can drive saving and undo
//! through the public methods (menu items, shortcuts). The state and every
//! command live in [`Session`], which this panel derefs to; `h2-studio`
//! draws the same session with Dear ImGui.
use crate::edit::Command;
use crate::layout::{BlockKind, FieldType, Role};
use crate::package::PackageForm;
use crate::render::{self, Viewport};
use crate::save::MapSource;
use crate::scenario::{Scenario, TagRef, Value};
use crate::session::Session;
use egui::{PointerButton, Sense};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

pub use crate::session::GROWTH_REFUSAL;

/// The scenario editor panel.
pub struct ScenarioEditor {
    session: Session,
    open_path: String,
    save_path: String,
    viewport: Option<Viewport>,
    texture_id: Option<egui::TextureId>,
    /// The session revisions last uploaded to the viewport.
    world_rev: Option<u64>,
    markers_rev: Option<u64>,
    filter: String,
    /// The "Save as package" window, when open.
    package_form: Option<PackageForm>,
    /// The typed image path in that window.
    image_path: String,
    /// The "Open package" window's list, when open.
    package_list: Option<Vec<content::Package>>,
}

impl Default for ScenarioEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for ScenarioEditor {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl DerefMut for ScenarioEditor {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

impl ScenarioEditor {
    pub fn new() -> Self {
        Self {
            session: Session::new(),
            open_path: String::new(),
            save_path: String::new(),
            viewport: None,
            texture_id: None,
            world_rev: None,
            markers_rev: None,
            filter: String::new(),
            package_form: None,
            image_path: String::new(),
            package_list: None,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Draw the panel.
    pub fn ui(&mut self, ui: &mut egui::Ui, render: &egui_wgpu::RenderState) {
        self.shortcuts(ui);
        egui::Panel::top("scenario_toolbar").show(ui, |ui| self.toolbar(ui));
        let ctx = ui.ctx().clone();
        self.package_windows(&ctx);
        if !self.is_open() {
            ui.label(self.status());
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
                    self.set_status(e.to_string());
                }
            }
            if !self.disc_maps().is_empty() {
                let mut chosen = None;
                egui::ComboBox::from_id_salt("scenario_disc_maps")
                    .selected_text("Disc map")
                    .show_ui(ui, |ui| {
                        for (name, s) in self.session.disc_maps() {
                            if ui.selectable_label(false, name).clicked() {
                                chosen = Some((name.clone(), s.clone()));
                            }
                        }
                    });
                if let Some((name, s)) = chosen {
                    if let Err(e) = self.open_named(s, &name) {
                        self.set_status(e.to_string());
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
                .add_enabled(self.is_open(), egui::Button::new(label))
                .clicked()
            {
                let path = PathBuf::from(self.save_path.trim());
                if let Err(e) = self.save_as(&path) {
                    self.set_status(e.to_string());
                }
            }
            ui.separator();
            if ui
                .add_enabled(self.is_open(), egui::Button::new("Save as package..."))
                .on_hover_text("A patch against the disc's map, with a manifest, in <data>/maps")
                .clicked()
            {
                self.package_form = Some(self.form_defaults());
                self.image_path.clear();
            }
            if ui
                .add_enabled(self.is_open(), egui::Button::new("Open package..."))
                .clicked()
            {
                self.package_list = Some(self.packages());
            }
        });
        ui.label(self.status());
        if let Some((dir, id)) = self.last_package() {
            ui.horizontal(|ui| {
                ui.weak(format!("package {dir}, MapId"));
                ui.monospace(id.to_string());
            });
        }
    }

    fn package_windows(&mut self, ctx: &egui::Context) {
        if let Some(mut form) = self.package_form.take() {
            let mut open = true;
            let mut save = false;
            egui::Window::new("Save as package")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    egui::Grid::new("package_form")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.text_edit_singleline(&mut form.name);
                            ui.end_row();
                            ui.label("Author");
                            ui.text_edit_singleline(&mut form.author);
                            ui.end_row();
                            ui.label("Description");
                            ui.text_edit_multiline(&mut form.description);
                            ui.end_row();
                            ui.label("Image (PNG/JPEG, 512 KB)");
                            ui.text_edit_singleline(&mut self.image_path);
                            ui.end_row();
                            ui.label("Directory");
                            ui.text_edit_singleline(&mut form.dir);
                            ui.end_row();
                        });
                    ui.checkbox(
                        &mut form.activate,
                        "Make it the active package for this map",
                    );
                    ui.weak(format!("in {}", self.maps_dir().display()));
                    if self.package_dir() == Some(form.dir.as_str()) {
                        ui.weak("replaces this package's patch");
                    }
                    save = ui.button("Save package").clicked();
                });
            if save {
                let image = self.image_path.trim();
                form.image = (!image.is_empty()).then(|| PathBuf::from(image));
                match self.save_package(&form) {
                    Ok(_) => open = false,
                    Err(e) => self.set_status(e.to_string()),
                }
            }
            if open {
                self.package_form = Some(form);
            }
        }
        if let Some(list) = self.package_list.take() {
            let mut open = true;
            let mut chosen = None;
            egui::Window::new("Open package")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    if list.is_empty() {
                        ui.label(format!(
                            "No packages for {} in {}",
                            self.map_name().unwrap_or("this map"),
                            self.maps_dir().display()
                        ));
                    }
                    for p in &list {
                        ui.horizontal(|ui| {
                            if ui.button("Open").clicked() {
                                chosen = Some(p.dir_name().to_owned());
                            }
                            ui.label(&p.manifest.package.name);
                            ui.weak(p.dir_name());
                            ui.monospace(&p.id.to_string()[..16]);
                        });
                    }
                });
            if let Some(dir) = chosen {
                match self.open_package(&dir) {
                    Ok(_) => open = false,
                    Err(e) => self.set_status(e.to_string()),
                }
            }
            if open {
                self.package_list = Some(list);
            }
        }
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter"));
        let Some(scenario) = self.session.scenario() else {
            return;
        };
        let filter = self.filter.to_ascii_lowercase();
        let mut pick = None;
        let mut action = None;
        let refusals: Vec<_> = scenario
            .blocks
            .iter()
            .map(|b| {
                (
                    self.remove_refusal(b.def.kind),
                    self.add_refusal(b.def.kind),
                )
            })
            .collect();
        ui.horizontal(|ui| {
            let sel = self.selected();
            let (remove, add) = match sel {
                Some((kind, _)) => (self.remove_refusal(kind), self.add_refusal(kind)),
                None => (Some("select an entry"), Some("select an entry")),
            };
            let r = ui.add_enabled(remove.is_none(), egui::Button::new("Delete"));
            if let Some(why) = remove {
                r.clone().on_disabled_hover_text(why);
            }
            if r.clicked()
                && let Some((kind, i)) = sel
            {
                action = Some(Action::Remove(kind, i));
            }
            let r = ui.add_enabled(add.is_none(), egui::Button::new("Duplicate"));
            if let Some(why) = add {
                r.clone().on_disabled_hover_text(why);
            }
            if r.clicked()
                && let Some((kind, i)) = sel
            {
                action = Some(Action::Add(kind, Some(i)));
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (b, (remove, add)) in scenario.blocks.iter().zip(&refusals) {
                let header = format!("{} ({})", b.def.label, b.len());
                let r = egui::CollapsingHeader::new(header)
                    .id_salt(b.def.kind)
                    .show(ui, |ui| {
                        for i in 0..b.len() {
                            let label = scenario.label(b.def.kind, i);
                            if !filter.is_empty() && !label.to_ascii_lowercase().contains(&filter) {
                                continue;
                            }
                            let on = self.selected() == Some((b.def.kind, i));
                            let r = ui.selectable_label(on, label);
                            if r.clicked() {
                                pick = Some((b.def.kind, i));
                            }
                            r.context_menu(|ui| {
                                entry_menu(ui, b.def.kind, Some(i), *remove, *add, &mut action);
                            });
                        }
                    });
                r.header_response.context_menu(|ui| {
                    entry_menu(ui, b.def.kind, None, *remove, *add, &mut action);
                });
            }
        });
        if let Some(p) = pick {
            self.select(Some(p));
            self.focus_selected();
        }
        let r = match action {
            Some(Action::Remove(kind, i)) => self.remove_entry(kind, i),
            Some(Action::Add(kind, like)) => self.add_entry(kind, like).map(|_| ()),
            None => Ok(()),
        };
        if let Err(e) = r {
            self.set_status(e.to_string());
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some((kind, index)) = self.selected() else {
            ui.label("Select a placement in the outliner or the viewport.");
            return;
        };
        let Some(scenario) = self.session.scenario() else {
            return;
        };
        let block = scenario.block(kind);
        ui.heading(scenario.label(kind, index));
        ui.label(format!(
            "{} #{index}, stride {:#x} ({})",
            block.def.label, block.def.stride, block.def.stride_evidence
        ));
        let named = scenario.names_of(kind, index).len();
        if named > 0 {
            ui.label(format!(
                "named by {named} scenario object name(s): scripts may create it by name; \
                 deleting it leaves the name naming nothing"
            ));
        }
        ui.separator();
        let mut edits = Vec::new();
        for f in block.def.fields {
            let Ok(value) = scenario.get(kind, index, f.name) else {
                continue;
            };
            ui.horizontal(|ui| {
                ui.label(format!("{:#x} {}", f.offset, f.name))
                    .on_hover_text(f.evidence);
                ui.add_enabled_ui(f.editable, |ui| {
                    if let Some(v) = value_editor(ui, scenario, kind, f, value) {
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
                self.set_status(e.to_string());
            }
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, render: &egui_wgpu::RenderState) {
        if !self.is_open() {
            return;
        }
        let viewport = self
            .viewport
            .get_or_insert_with(|| Viewport::new(&render.device));
        if self.world_rev != Some(self.session.world_rev()) {
            viewport.set_world(&render.device, self.session.world().0);
            self.world_rev = Some(self.session.world_rev());
        }
        if self.markers_rev != Some(self.session.markers_rev()) {
            viewport.set_markers(
                &render.device,
                &render::marker_vertices(self.session.markers(), self.session.selected()),
            );
            self.markers_rev = Some(self.session.markers_rev());
        }
        let avail = ui.available_size().max(egui::vec2(64.0, 64.0));
        let ppp = ui.ctx().pixels_per_point();
        let px = [(avail.x * ppp) as u32, (avail.y * ppp) as u32];
        let resized = viewport.render(
            &render.device,
            &render.queue,
            self.session.camera(),
            px,
        );
        let view = viewport.color_view().expect("rendered");
        let tex = {
            let mut r = render.renderer.write();
            match self.texture_id {
                Some(id) if resized => {
                    r.update_egui_texture_from_wgpu_texture(
                        &render.device,
                        view,
                        egui_wgpu::wgpu::FilterMode::Linear,
                        id,
                    );
                    id
                }
                Some(id) => id,
                None => {
                    let id = r.register_native_texture(
                        &render.device,
                        view,
                        egui_wgpu::wgpu::FilterMode::Linear,
                    );
                    self.texture_id = Some(id);
                    id
                }
            }
        };
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
            self.camera_mut().orbit(-d.x * 0.008, d.y * 0.008);
        }
        if response.dragged_by(PointerButton::Middle) {
            let d = response.drag_delta();
            self.camera_mut().pan(d.x, d.y, rect.height());
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
                self.camera_mut().zoom((-scroll * 0.002).exp());
            }
            if keys.iter().any(|&k| k != 0.0) {
                let speed = self.camera().distance.max(5.0) * dt;
                self.camera_mut()
                    .fly(keys[0] * speed, keys[1] * speed, keys[2] * speed);
                ui.ctx().request_repaint();
            }
        }

        // Selection and dragging with the primary button.
        if response.clicked_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let (u, v) = uv(p);
            let hit = self.pick(&self.camera().ray(u, v, aspect));
            self.select(hit);
        }
        if response.drag_started_by(PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let (u, v) = uv(p);
            let ray = self.camera().ray(u, v, aspect);
            if let Some(hit) = self.pick(&ray) {
                self.select(Some(hit));
            }
            let vertical = ui.input(|i| i.modifiers.shift);
            self.begin_drag(&ray, vertical);
        }
        if response.dragged_by(PointerButton::Primary)
            && self.dragging()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (u, v) = uv(p);
            let ray = self.camera().ray(u, v, aspect);
            self.drag_to(&ray);
        }
        if response.drag_stopped() {
            self.end_drag();
        }
    }
}

/// An outliner action, applied after the outliner is drawn.
#[derive(Clone, Copy, Debug)]
enum Action {
    Remove(BlockKind, usize),
    /// Add to a block: a copy of an entry, or a new entry.
    Add(BlockKind, Option<usize>),
}

/// The context menu of an outliner entry (`index`) or block header.
fn entry_menu(
    ui: &mut egui::Ui,
    kind: BlockKind,
    index: Option<usize>,
    remove: Option<&'static str>,
    add: Option<&'static str>,
    action: &mut Option<Action>,
) {
    let mut item = |ui: &mut egui::Ui, text: &str, refusal: Option<&'static str>, a: Action| {
        let r = ui.add_enabled(refusal.is_none(), egui::Button::new(text));
        if let Some(why) = refusal {
            r.clone().on_disabled_hover_text(why);
        }
        if r.clicked() {
            *action = Some(a);
            ui.close();
        }
    };
    if let Some(i) = index {
        item(ui, "Duplicate", add, Action::Add(kind, Some(i)));
        item(ui, "Delete", remove, Action::Remove(kind, i));
    }
    item(
        ui,
        "Add new at the camera focus",
        add,
        Action::Add(kind, None),
    );
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
