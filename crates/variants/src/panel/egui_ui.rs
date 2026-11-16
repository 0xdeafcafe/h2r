//! The egui drawing of [`VariantEditor`] (feature `egui`): the toolbar,
//! the list and the settings form. Everything it changes goes through the
//! editor's plain methods.

use super::*;
use crate::record::{self, Kind};

impl VariantEditor {
    fn report<T>(&mut self, r: Result<T, EditError>) {
        if let Err(e) = r {
            self.status = Some((true, e.to_string()));
        }
    }

    /// Draws the panel.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        ui.separator();
        egui::Panel::left("variants_list")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.list(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.form(ui));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let can_save = self.is_dirty() && !self.is_read_only() && self.store.is_some();
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .clicked()
            {
                let r = self.save();
                self.report(r);
            }
            if ui
                .add_enabled(self.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(self.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                self.redo();
            }
            let changed = self.doc.as_ref().is_some_and(|d| d.variant != d.base);
            if ui
                .add_enabled(changed, egui::Button::new("Revert"))
                .clicked()
            {
                self.revert();
            }
            ui.separator();
            egui::ComboBox::from_id_salt("variants_new_type")
                .selected_text(self.new_type.label())
                .show_ui(ui, |ui| {
                    for t in GameType::ALL {
                        ui.selectable_value(&mut self.new_type, t, t.label());
                    }
                });
            if ui.button("New").clicked() {
                self.new_variant(self.new_type);
            }
            if ui
                .add_enabled(self.current().is_some(), egui::Button::new("Export"))
                .on_hover_text("Copy the variant as a portable .h2variant file to the clipboard")
                .clicked()
                && let Some(t) = self.export_current()
            {
                ui.ctx().copy_text(t);
                self.status = Some((false, "copied to the clipboard".into()));
            }
            let saved = matches!(self.origin(), Some(Origin::Saved(_)));
            if ui.add_enabled(saved, egui::Button::new("Delete")).clicked() {
                let r = self.delete();
                self.report(r);
            }
        });
        ui.collapsing("Import a portable variant", |ui| {
            ui.add(egui::TextEdit::multiline(&mut self.import_text).desired_rows(3));
            if ui.button("Import").clicked() {
                let text = std::mem::take(&mut self.import_text);
                let r = self.import(&text);
                if r.is_err() {
                    self.import_text = text;
                }
                self.report(r);
            }
        });
        if let Some((err, s)) = &self.status {
            let color = if *err {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.colored_label(color, s);
        }
        ui.weak(format!("Schema: {}", self.schema.source));
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Saved");
            if self.store.is_none() {
                ui.weak("no save storage");
            } else if self.saved.is_empty() {
                ui.weak("no saved variants");
            }
            for s in &self.saved {
                let open = self.origin() == Some(&Origin::Saved(s.dir.clone()));
                let v = &s.contents.variant;
                let text = format!("{}  ({})", v.name(), v.game_type().label());
                if ui
                    .selectable_label(open, text)
                    .on_hover_text(&s.save_name)
                    .clicked()
                {
                    pick = Some(Origin::Saved(s.dir.clone()));
                }
            }
            ui.separator();
            ui.heading("Built-in");
            if self.builtins.is_empty() {
                ui.weak("none (no disc)");
            }
            for (i, v) in self.builtins.iter().enumerate() {
                let open = self.origin() == Some(&Origin::Builtin(i));
                let text = format!("{}  ({})", strip_icon(&v.name()), v.game_type().label());
                if ui
                    .selectable_label(open, text)
                    .on_hover_text("Built-in: read-only")
                    .clicked()
                {
                    pick = Some(Origin::Builtin(i));
                }
            }
        });
        if let Some(p) = pick {
            if self.is_dirty() && self.origin() != Some(&p) {
                self.pending = Some(p);
            } else {
                self.open_origin(p);
            }
        }
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        if let Some(p) = self.pending.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "The open variant has unsaved changes.",
                );
                if ui.button("Discard them").clicked() {
                    self.open_origin(p);
                }
                if ui.button("Keep editing").clicked() {
                    self.pending = None;
                }
            });
            ui.separator();
        }
        let Some(doc) = &self.doc else {
            ui.weak("Pick a variant on the left, or make a new one.");
            return;
        };
        let v = doc.variant.clone();
        let ty = v.game_type();
        let read_only = self.is_read_only();

        if read_only {
            ui.horizontal(|ui| {
                ui.label("Built-in variant (cannot edit). Copy as");
                ui.text_edit_singleline(&mut self.copy_name);
                if ui.button("Copy").clicked() {
                    let name = self.copy_name.clone();
                    let r = self.copy_builtin(&name);
                    self.report(r);
                }
            });
        }
        let mut name = v.name();
        ui.horizontal(|ui| {
            ui.label("Name");
            let r = ui.add_enabled(
                !read_only,
                egui::TextEdit::singleline(&mut name).char_limit(record::MAX_NAME_UNITS),
            );
            if r.changed() {
                let res = self.set_name(&name);
                self.report(res);
            }
            ui.label(format!("Game type: {}", ty.label()));
            if self.is_dirty() {
                ui.weak("(unsaved)");
            }
        });

        let issues = self.schema.validate(&v);
        if !issues.is_empty() {
            ui.collapsing(
                format!("The game would not accept this ({})", issues.len()),
                |ui| {
                    for i in &issues {
                        ui.colored_label(ui.visuals().error_fg_color, i.to_string());
                    }
                },
            );
        }
        let diff = self.schema.diff_from_defaults(&v);
        ui.collapsing(
            format!("Changes from the {} defaults ({})", ty.label(), diff.len()),
            |ui| {
                for (id, was, now) in &diff {
                    ui.label(format!(
                        "{}: {} -> {}",
                        self.schema.title(*id),
                        self.value_text(*id, *was),
                        self.value_text(*id, *now)
                    ));
                }
            },
        );
        ui.separator();

        let defaults = self.schema.new_variant(ty);
        let mut changes = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("variants_form")
            .show(ui, |ui| {
                ui.add_enabled_ui(!read_only, |ui| {
                    for page in self.schema.pages_for(ty) {
                        egui::CollapsingHeader::new(&page.title)
                            .id_salt(("variants_page", page.id))
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::Grid::new(("variants_grid", page.id))
                                    .num_columns(2)
                                    .striped(true)
                                    .show(ui, |ui| {
                                        for id in &page.settings {
                                            if let Some(x) =
                                                self.setting_row(ui, *id, &v, &defaults)
                                            {
                                                changes.push((*id, x));
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                    }
                });
            });
        for (id, x) in changes {
            let r = self.set_setting(id, x);
            self.report(r);
        }
    }

    /// One row; returns a new value if the user changed it.
    fn setting_row(
        &self,
        ui: &mut egui::Ui,
        id: SettingId,
        v: &GameVariant,
        defaults: &GameVariant,
    ) -> Option<i32> {
        let value = v.get(id)?;
        let def = self.schema.settings.get(&id);
        let title = self.schema.title(id);
        let changed = defaults.get(id) != Some(value);
        let mut text = egui::RichText::new(title);
        if changed {
            text = text.strong();
        }
        let help = def
            .map(|d| d.help.as_str())
            .filter(|h| !h.is_empty())
            .unwrap_or("No help text.");
        let default_text = defaults
            .get(id)
            .map(|d| self.value_text(id, d))
            .unwrap_or_default();
        ui.label(text)
            .on_hover_text(format!("{help}\n\nDefault: {default_text}"));
        let mut out = None;
        match def.filter(|d| !d.choices.is_empty()) {
            Some(d) => {
                let selected = self.value_text(id, value);
                let selected = if d.choices.iter().any(|c| c.value == value) {
                    selected
                } else {
                    format!("{value} (not offered)")
                };
                egui::ComboBox::from_id_salt(("variants_setting", id.0))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for c in &d.choices {
                            if ui.selectable_label(c.value == value, &c.label).clicked()
                                && c.value != value
                            {
                                out = Some(c.value);
                            }
                        }
                    });
            }
            None => {
                let kind = record::field(id)?.kind;
                if matches!(kind, Kind::Bit16(_) | Kind::Bit32(_) | Kind::Bool8) {
                    let mut on = value != 0;
                    if ui.checkbox(&mut on, "").changed() {
                        out = Some(i32::from(on));
                    }
                } else {
                    let (lo, hi) = match kind {
                        Kind::U8 => (0, 255),
                        Kind::I16 => (i16::MIN.into(), i16::MAX.into()),
                        _ => (i32::MIN, i32::MAX),
                    };
                    let mut x = value;
                    if ui
                        .add(egui::DragValue::new(&mut x).range(lo..=hi))
                        .changed()
                        && x != value
                    {
                        out = Some(x);
                    }
                }
            }
        }
        out
    }
}
