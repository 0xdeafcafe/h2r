//! The game variant editor panel.
//!
//! A list of saved and built-in variants on the left, the open variant's
//! settings on the right, grouped into the game's pages, with each
//! setting's help on hover, the changes from the game type's defaults and
//! anything the game would reject. Built-ins are read-only: as in the game,
//! editing one starts from a named copy. Every edit can be undone.
//!
//! The editing operations are plain methods so the shell (menus, keyboard
//! shortcuts) and tests can drive them without a frame; [`VariantEditor::ui`]
//! only draws and calls them.

use std::path::PathBuf;

use crate::portable::{self, PortableError};
use crate::record::{self, GameType, GameVariant, Kind, RecordError, SettingId};
use crate::save::{SaveError, SaveStore, SavedVariant};
use crate::schema::{Issue, Schema};

/// Where the open variant came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Built-in `i` of the schema (read-only).
    Builtin(usize),
    /// A save directory.
    Saved(PathBuf),
    /// Not saved yet (new, a copy of a built-in, or imported).
    New,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("no variant is open")]
    NoVariant,
    #[error("built-in variants cannot be edited; make a copy")]
    ReadOnly,
    #[error("no save storage is configured")]
    NoStore,
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Save(#[from] SaveError),
    #[error(transparent)]
    Import(#[from] PortableError),
}

/// What the last edit changed, so typing a name is one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Name,
    Setting(SettingId),
}

#[derive(Debug, Clone)]
struct Doc {
    origin: Origin,
    variant: GameVariant,
    /// The variant as last loaded or saved.
    base: GameVariant,
    undo: Vec<GameVariant>,
    redo: Vec<GameVariant>,
    last: Option<Target>,
}

const MAX_UNDO: usize = 256;

pub struct VariantEditor {
    schema: Schema,
    store: Option<SaveStore>,
    builtins: Vec<GameVariant>,
    saved: Vec<SavedVariant>,
    doc: Option<Doc>,
    /// A list entry the user picked while the open variant had changes.
    pending: Option<Origin>,
    copy_name: String,
    new_type: GameType,
    import_text: String,
    status: Option<(bool, String)>,
}

impl VariantEditor {
    /// An editor over `schema` (from the disc, or [`Schema::fallback`]) and
    /// the saves in `store`, if any.
    pub fn new(schema: Schema, store: Option<SaveStore>) -> Self {
        let builtins = schema
            .builtins
            .iter()
            .map(|b| {
                let mut v = schema
                    .build(b)
                    .unwrap_or_else(|_| schema.new_variant(b.game_type));
                v.set_builtin_flag(true);
                v
            })
            .collect();
        let mut e = VariantEditor {
            schema,
            store,
            builtins,
            saved: Vec::new(),
            doc: None,
            pending: None,
            copy_name: String::new(),
            new_type: GameType::Slayer,
            import_text: String::new(),
            status: None,
        };
        e.refresh();
        e
    }

    /// Re-reads the saves.
    pub fn refresh(&mut self) {
        if let Some(store) = &self.store {
            match store.list() {
                Ok(l) => self.saved = l,
                Err(err) => self.status = Some((true, format!("cannot list saves: {err}"))),
            }
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn builtins(&self) -> &[GameVariant] {
        &self.builtins
    }

    pub fn saved(&self) -> &[SavedVariant] {
        &self.saved
    }

    /// The open variant.
    pub fn current(&self) -> Option<&GameVariant> {
        self.doc.as_ref().map(|d| &d.variant)
    }

    pub fn origin(&self) -> Option<&Origin> {
        self.doc.as_ref().map(|d| &d.origin)
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self.origin(), Some(Origin::Builtin(_)))
    }

    /// The open variant has changes not written to its save (a variant
    /// that was never saved counts as changed).
    pub fn is_dirty(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.origin == Origin::New || d.variant != d.base)
    }

    /// The last status message, `true` for an error.
    pub fn status(&self) -> Option<(bool, &str)> {
        self.status.as_ref().map(|(e, s)| (*e, s.as_str()))
    }

    fn open(&mut self, origin: Origin, v: GameVariant) {
        self.doc = Some(Doc {
            origin,
            base: v.clone(),
            variant: v,
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
        });
        self.pending = None;
    }

    pub fn open_builtin(&mut self, i: usize) {
        if let Some(v) = self.builtins.get(i).cloned() {
            self.copy_name = format!("{} copy", strip_icon(&v.name()));
            self.open(Origin::Builtin(i), v);
        }
    }

    pub fn open_saved(&mut self, i: usize) {
        if let Some(s) = self.saved.get(i) {
            let (dir, v, signed) = (s.dir.clone(), s.contents.variant.clone(), s.contents.signed);
            self.open(Origin::Saved(dir), v);
            if !signed {
                self.status = Some((
                    true,
                    "this save's signature does not match the signing key; saving re-signs it"
                        .into(),
                ));
            }
        }
    }

    /// A new, unsaved variant of `ty` with the type's defaults.
    pub fn new_variant(&mut self, ty: GameType) {
        let mut v = self.schema.new_variant(ty);
        let _ = v.set_name(&format!("New {}", ty.label()));
        self.open(Origin::New, v);
    }

    /// Copies the open built-in under `name`, as the game's Edit does
    /// (`0x212b2c`): the built-in flag cleared, the new name set.
    pub fn copy_builtin(&mut self, name: &str) -> Result<(), EditError> {
        let doc = self.doc.as_ref().ok_or(EditError::NoVariant)?;
        let mut v = doc.variant.clone();
        v.set_builtin_flag(false);
        v.set_name(name)?;
        self.open(Origin::New, v);
        Ok(())
    }

    fn edit(
        &mut self,
        target: Target,
        f: impl FnOnce(&mut GameVariant) -> Result<(), RecordError>,
    ) -> Result<(), EditError> {
        let read_only = self.is_read_only();
        let doc = self.doc.as_mut().ok_or(EditError::NoVariant)?;
        if read_only {
            return Err(EditError::ReadOnly);
        }
        let before = doc.variant.clone();
        f(&mut doc.variant)?;
        if doc.variant == before {
            return Ok(());
        }
        if doc.last != Some(target) || target != Target::Name {
            doc.undo.push(before);
            if doc.undo.len() > MAX_UNDO {
                doc.undo.remove(0);
            }
        }
        doc.redo.clear();
        doc.last = Some(target);
        Ok(())
    }

    pub fn set_setting(&mut self, id: SettingId, value: i32) -> Result<(), EditError> {
        self.edit(Target::Setting(id), |v| v.set(id, value))
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), EditError> {
        self.edit(Target::Name, |v| v.set_name(name))
    }

    pub fn can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| !d.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| !d.redo.is_empty())
    }

    pub fn undo(&mut self) -> bool {
        let Some(d) = self.doc.as_mut() else {
            return false;
        };
        let Some(prev) = d.undo.pop() else {
            return false;
        };
        d.redo.push(std::mem::replace(&mut d.variant, prev));
        d.last = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(d) = self.doc.as_mut() else {
            return false;
        };
        let Some(next) = d.redo.pop() else {
            return false;
        };
        d.undo.push(std::mem::replace(&mut d.variant, next));
        d.last = None;
        true
    }

    /// Back to the variant as last loaded or saved (undoable).
    pub fn revert(&mut self) {
        if let Some(d) = self.doc.as_mut()
            && d.variant != d.base
        {
            let base = d.base.clone();
            d.undo.push(std::mem::replace(&mut d.variant, base));
            d.redo.clear();
            d.last = None;
        }
    }

    /// Writes the open variant: a saved one in place (a new save if its
    /// name changed), a new one as a new save. Built-ins cannot be saved.
    pub fn save(&mut self) -> Result<(), EditError> {
        let doc = self.doc.as_ref().ok_or(EditError::NoVariant)?;
        let store = self.store.as_ref().ok_or(EditError::NoStore)?;
        let saved = match &doc.origin {
            Origin::Builtin(_) => return Err(EditError::ReadOnly),
            Origin::New => store.create(&doc.variant)?,
            Origin::Saved(dir) => {
                let old = self
                    .saved
                    .iter()
                    .find(|s| &s.dir == dir)
                    .cloned()
                    .map_or_else(|| store.read(dir), Ok)?;
                store.update(&old, &doc.variant)?
            }
        };
        let doc = self.doc.as_mut().unwrap();
        doc.origin = Origin::Saved(saved.dir.clone());
        doc.base = doc.variant.clone();
        self.status = Some((false, format!("saved as \"{}\"", saved.save_name)));
        self.refresh();
        Ok(())
    }

    /// Deletes the open saved variant.
    pub fn delete(&mut self) -> Result<(), EditError> {
        let store = self.store.as_ref().ok_or(EditError::NoStore)?;
        let Some(Origin::Saved(dir)) = self.origin().cloned() else {
            return Err(EditError::NoVariant);
        };
        let s = store.read(&dir)?;
        store.delete(&s)?;
        self.doc = None;
        self.refresh();
        Ok(())
    }

    /// The open variant as a portable file.
    pub fn export_current(&self) -> Option<String> {
        let mut v = self.current()?.clone();
        v.set_builtin_flag(false);
        Some(portable::export(&v))
    }

    /// Opens a portable file as a new, unsaved variant.
    pub fn import(&mut self, text: &str) -> Result<(), EditError> {
        let (mut v, problems) = portable::import(text)?;
        v.set_builtin_flag(false);
        self.open(Origin::New, v);
        if !problems.is_empty() {
            self.status = Some((
                true,
                format!(
                    "imported; the game would reject {} field(s) until fixed",
                    problems.len()
                ),
            ));
        }
        Ok(())
    }

    /// Findings for the open variant.
    pub fn issues(&self) -> Vec<Issue> {
        self.current()
            .map(|v| self.schema.validate(v))
            .unwrap_or_default()
    }

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

    fn open_origin(&mut self, p: Origin) {
        match p {
            Origin::Builtin(i) => self.open_builtin(i),
            Origin::Saved(dir) => {
                if let Some(i) = self.saved.iter().position(|s| s.dir == dir) {
                    self.open_saved(i);
                }
            }
            Origin::New => {}
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

    fn value_text(&self, id: SettingId, value: i32) -> String {
        match self.schema.value_label(id, value) {
            Some(l) => l.to_string(),
            None => value.to_string(),
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

/// A built-in's name without the trailing icon glyph (U+E000..U+E0FF) the
/// game's list draws as a padlock or similar.
fn strip_icon(name: &str) -> String {
    name.trim_end_matches(|c: char| ('\u{e000}'..='\u{e0ff}').contains(&c))
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::{CONTENT_TYPES, XbeNames};
    use crate::schema::{Builtin, Choice, Page, SettingDef};
    use std::collections::BTreeMap;

    /// A small invented schema: one page with team play and a starting
    /// weapon, Slayer defaults, and one built-in.
    fn schema() -> Schema {
        let mut settings = BTreeMap::new();
        let choice = |value, label: &str| Choice {
            value,
            label: label.into(),
            default: value == 0,
        };
        settings.insert(
            SettingId(91),
            SettingDef {
                id: SettingId(91),
                title: "Team".into(),
                help: "Teams or not".into(),
                choices: vec![choice(0, "Off"), choice(1, "On")],
            },
        );
        settings.insert(
            SettingId(89),
            SettingDef {
                id: SettingId(89),
                title: "Weapon".into(),
                help: String::new(),
                choices: vec![choice(0, "A"), choice(3, "B"), choice(9, "C")],
            },
        );
        let mut defaults = BTreeMap::new();
        defaults.insert(GameType::Slayer, vec![(SettingId(89), 3)]);
        Schema {
            settings,
            pages: vec![Page {
                id: 12,
                title: "Weapons".into(),
                settings: vec![SettingId(91), SettingId(89)],
            }],
            defaults,
            builtins: vec![Builtin {
                name: "Classic \u{e008}".into(),
                game_type: GameType::Slayer,
                description: 4,
                pairs: vec![(SettingId(91), 1)],
            }],
            source: "test".into(),
        }
    }

    fn store(tag: &str) -> SaveStore {
        let root =
            std::env::temp_dir().join(format!("variants-panel-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        SaveStore::new(
            root,
            Some(XbeNames {
                file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
                labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
            }),
        )
    }

    #[test]
    fn builtins_are_read_only_and_copy_on_edit() {
        let mut e = VariantEditor::new(schema(), None);
        assert_eq!(e.builtins().len(), 1);
        e.open_builtin(0);
        assert!(e.is_read_only() && !e.is_dirty());
        let v = e.current().unwrap();
        assert!(v.builtin_flag() && v.team_play());
        assert_eq!(v.get(SettingId(89)), Some(3), "the type's defaults first");
        assert_eq!(v.description(), 4);
        assert!(matches!(
            e.set_setting(SettingId(89), 9),
            Err(EditError::ReadOnly)
        ));
        assert!(matches!(
            e.save(),
            Err(EditError::NoStore | EditError::ReadOnly)
        ));

        e.copy_builtin("Mine").unwrap();
        assert_eq!(e.origin(), Some(&Origin::New));
        assert!(e.is_dirty() && !e.is_read_only());
        let v = e.current().unwrap();
        assert!(!v.builtin_flag());
        assert_eq!(v.name(), "Mine");
        assert!(v.team_play(), "settings come along");
    }

    #[test]
    fn undo_and_redo() {
        let mut e = VariantEditor::new(schema(), None);
        e.new_variant(GameType::Slayer);
        let start = e.current().unwrap().clone();
        e.set_setting(SettingId(89), 9).unwrap();
        e.set_setting(SettingId(91), 1).unwrap();
        // Typing a name is one step.
        e.set_name("A").unwrap();
        e.set_name("AB").unwrap();
        e.set_name("ABC").unwrap();
        assert!(e.undo());
        assert_eq!(e.current().unwrap().name(), start.name());
        assert!(e.undo());
        assert!(!e.current().unwrap().team_play());
        assert!(e.redo());
        assert!(e.current().unwrap().team_play());
        assert!(e.undo() && e.undo());
        assert_eq!(e.current().unwrap(), &start);
        assert!(!e.undo() && e.can_redo());
        // A new edit drops the redo history.
        e.set_setting(SettingId(89), 0).unwrap();
        assert!(!e.can_redo());
        // An edit that changes nothing is not a step.
        e.set_setting(SettingId(89), 0).unwrap();
        assert!(e.undo() && !e.can_undo());
    }

    #[test]
    fn save_writes_and_tracks_dirty() {
        let st = store("save");
        let root = st.root.clone();
        let mut e = VariantEditor::new(schema(), Some(st));
        e.open_builtin(0);
        e.copy_builtin("Mine").unwrap();
        e.save().unwrap();
        assert!(!e.is_dirty());
        assert_eq!(e.saved().len(), 1);
        assert_eq!(e.saved()[0].save_name, "L1: Mine");
        assert!(matches!(e.origin(), Some(Origin::Saved(_))));

        e.set_setting(SettingId(89), 9).unwrap();
        assert!(e.is_dirty());
        e.revert();
        assert!(!e.is_dirty());
        assert!(e.undo() && e.is_dirty());
        e.save().unwrap();
        assert_eq!(e.saved()[0].contents.variant.get(SettingId(89)), Some(9));

        // A value the game rejects is not written.
        e.set_setting(SettingId(19), 3).unwrap();
        assert!(matches!(
            e.save(),
            Err(EditError::Save(SaveError::Rejected(_)))
        ));
        e.undo();

        // Export and import round trip into a new, unsaved variant.
        let text = e.export_current().unwrap();
        e.import(&text).unwrap();
        assert_eq!(e.origin(), Some(&Origin::New));
        assert_eq!(e.current().unwrap().get(SettingId(89)), Some(9));

        e.open_saved(0);
        e.delete().unwrap();
        assert!(e.saved().is_empty() && e.current().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_and_issues() {
        let mut e = VariantEditor::new(schema(), None);
        e.new_variant(GameType::Slayer);
        assert!(e.issues().is_empty());
        e.set_setting(SettingId(89), 5).unwrap();
        let diff = e.schema().diff_from_defaults(e.current().unwrap());
        assert_eq!(diff, vec![(SettingId(89), 3, 5)]);
        assert_eq!(
            e.issues(),
            vec![Issue::NotOffered {
                id: SettingId(89),
                value: 5
            }]
        );
    }

    /// Draw frames headlessly (no window): every state of the panel lays out
    /// without panicking, with the disc schema's shape and the fallback.
    #[test]
    fn draws_headless() {
        for s in [schema(), Schema::fallback()] {
            let mut e = VariantEditor::new(s, Some(store("draw")));
            let ctx = egui::Context::default();
            let frame = |e: &mut VariantEditor| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| e.ui(ui));
                // No renderer here to take the font atlas.
                out.textures_delta.clear();
                assert!(!out.shapes.is_empty());
            };
            frame(&mut e);
            if !e.builtins().is_empty() {
                e.open_builtin(0);
                frame(&mut e);
            }
            for t in GameType::ALL {
                e.new_variant(t);
                frame(&mut e);
            }
            e.pending = Some(Origin::New);
            frame(&mut e);
        }
    }

    #[test]
    fn icons_are_stripped_from_names() {
        assert_eq!(strip_icon("Slayer \u{e008}"), "Slayer");
        assert_eq!(strip_icon("Plain"), "Plain");
    }
}
