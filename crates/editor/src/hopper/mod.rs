//! The hopper (playlist) tool: edits `multiplayer_hopper.dat`, the 16
//! playlists the game downloads after signing in to Live
//! (docs/live/live-admin.md, docs/live/live-matchmaking.md). The native counterpart
//! of the web admin's playlist builder (`web/src/admin/HopperBuilder.tsx`):
//! the same fields, edits, validation and presets, on a local file instead
//! of the server's copy.
//!
//! The editing logic and file handling are UI-free and tested in the
//! `hopper-edit` crate (shared with h2-studio); this module is the egui
//! panel, the file dialog and the hand-off to the variant tool.

pub use hopper_edit::{catalog, choices, model, presets};

use std::path::{Path, PathBuf};

use hopper_edit::EditView;
use live_proto::hopper::{
    HOPPERS, HopperFile, MAX_DESCRIPTION, MAX_NAME, MAX_RECORDS, MAX_STRING, MAX_TITLE,
};

use variants::GameVariant;

use self::catalog::Catalog;
use self::choices::{Choice, Choices, variant_label};
use self::model::{EditHopper, EditRecord, MAX_WEIGHT};
use self::presets::{PRESETS, Resolution};
use crate::dialog::{FileDialog, Filter, Picked};
use crate::tool::{Handoff, Tool, ToolContext, ToolError};

pub use hopper_edit::{Format, HopperSession};

impl From<hopper_edit::Error> for ToolError {
    fn from(e: hopper_edit::Error) -> Self {
        match e {
            hopper_edit::Error::Invalid(m) => ToolError::Invalid(m),
            hopper_edit::Error::Io(e) => ToolError::Io(e),
        }
    }
}

/// Reads either form of the file.
pub fn read_file(bytes: &[u8]) -> Result<(HopperFile, Format), ToolError> {
    Ok(hopper_edit::read_file(bytes)?)
}

/// The bytes a save writes: the envelope (refused when larger than the
/// game's download buffer) or the raw image.
pub fn write_file(file: &HopperFile, format: Format) -> Result<Vec<u8>, ToolError> {
    Ok(hopper_edit::write_file(file, format)?)
}

/// The hopper tool: the egui panel over a [`HopperSession`] (which it
/// derefs to), with the file dialog and the hand-off to the variant tool.
pub struct HopperTool {
    session: HopperSession,
    selected: usize,
    preset: Option<usize>,
    /// The typed `.h2variant` path.
    variant_path: String,
    /// Browses for `.h2variant` files; without one, paths are typed.
    dialog: Option<Box<dyn FileDialog>>,
    handoff: Option<Handoff>,
}

impl Default for HopperTool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for HopperTool {
    type Target = HopperSession;
    fn deref(&self) -> &HopperSession {
        &self.session
    }
}

impl std::ops::DerefMut for HopperTool {
    fn deref_mut(&mut self) -> &mut HopperSession {
        &mut self.session
    }
}

impl HopperTool {
    pub fn new() -> Self {
        HopperTool {
            session: HopperSession::new(),
            selected: 0,
            preset: None,
            variant_path: String::new(),
            dialog: None,
            handoff: None,
        }
    }

    /// Browses for `.h2variant` files with `dialog` (the system's in the
    /// app) next to the typed path.
    pub fn set_file_dialog(&mut self, dialog: Box<dyn FileDialog>) {
        self.dialog = Some(dialog);
    }

    /// Links `v` to an entry, as one undo step; refuses a variant the game
    /// would reject.
    pub fn set_entry_variant(
        &mut self,
        hopper: usize,
        entry: usize,
        v: &GameVariant,
    ) -> Result<(), ToolError> {
        Ok(self.session.set_entry_variant(hopper, entry, v)?)
    }

    /// Starts from bytes read from `path` (either form).
    pub fn open_bytes(&mut self, bytes: &[u8], path: Option<PathBuf>) -> Result<(), ToolError> {
        self.session.open_bytes(bytes, path)?;
        self.selected = 0;
        self.preset = None;
        Ok(())
    }

    /// Starts from a blank image.
    pub fn new_blank(&mut self) {
        self.session.new_blank();
        self.selected = 0;
        self.preset = None;
    }

    /// The edited image, or why it cannot be written.
    pub fn build(&self) -> Result<HopperFile, ToolError> {
        Ok(self.session.build()?)
    }

    /// Writes the edits to `path` (see [`HopperSession::save_to`]).
    pub fn save_to(&mut self, path: &Path) -> Result<usize, ToolError> {
        Ok(self.session.save_to(path)?)
    }

    /// Asks the shell to open entry `entry`'s variant in the variant
    /// editor (as a new copy there).
    pub fn edit_entry_variant(&mut self, hopper: usize, entry: usize) -> bool {
        match self.session.entry_variant(hopper, entry) {
            Some(v) => {
                self.handoff = Some(Handoff::EditVariant(Box::new(v)));
                true
            }
            None => false,
        }
    }

    /// Asks the dialog for a `.h2variant` file and loads it; `None` when
    /// cancelled.
    fn browse_variant_file(&mut self) -> Option<Result<String, String>> {
        let dialog = self.dialog.as_mut()?;
        let filters = [Filter::new(
            "Game variant",
            &[variants::portable::EXTENSION],
        )];
        let start = Path::new(&self.variant_path);
        let start = (!self.variant_path.trim().is_empty()).then_some(start);
        match dialog.open_file("Open a game variant", &filters, start) {
            Picked::Path(p) => {
                self.variant_path = p.display().to_string();
                Some(self.session.load_variant_file(&p))
            }
            Picked::Cancelled => None,
            Picked::Unavailable => Some(Err(
                "no file dialog: type the .h2variant path and press Load".into(),
            )),
        }
    }
}

impl Tool for HopperTool {
    fn title(&self) -> &str {
        "Playlists (hoppers)"
    }

    fn is_dirty(&self) -> bool {
        self.session.is_dirty()
    }

    fn path(&self) -> Option<&Path> {
        self.session.path()
    }

    fn extensions(&self) -> &[&str] {
        &["dat"]
    }

    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let format = self.session.open_path(path)?;
        self.selected = 0;
        self.preset = None;
        cx.info(format!("opened {} ({})", path.display(), format.describe()));
        Ok(())
    }

    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let path = self.path().map(Path::to_owned).ok_or(ToolError::NoPath)?;
        self.save_as(&path, cx)
    }

    fn save_as(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let n = self.save_to(path)?;
        cx.info(format!("saved {} ({n} bytes)", path.display()));
        Ok(())
    }

    fn can_undo(&self) -> bool {
        self.session.can_undo()
    }

    fn can_redo(&self) -> bool {
        self.session.can_redo()
    }

    fn undo(&mut self) -> bool {
        self.session.undo()
    }

    fn redo(&mut self) -> bool {
        self.session.redo()
    }

    fn take_handoff(&mut self) -> Option<Handoff> {
        self.handoff.take()
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>) {
        self.session.refresh_catalog(cx.source);
        if !self.session.is_open() {
            ui.heading("Playlists");
            ui.label(
                "Open a multiplayer_hopper.dat (the console's raw image or a downloaded \
                 envelope) with Open in the top bar, or start from a blank file.",
            );
            if ui.button("New blank file").clicked() {
                self.new_blank();
            }
            return;
        }
        let catalog_note = self.session.catalog_note();
        self.session.refresh_variants(cx.source);
        let browse = self.dialog.is_some();
        let (selected, preset, variant_path) =
            (&mut self.selected, &mut self.preset, &mut self.variant_path);
        let (action, error) = self
            .session
            .edit_with(|hoppers, view| {
                let mut panel = Panel {
                    selected,
                    preset,
                    catalog: view.catalog,
                    catalog_note,
                    choices: view.choices,
                    variant_path,
                    browse,
                    action: None,
                    error: None,
                };
                panel.show(ui, hoppers, view);
                // One undo step per change; a run of edits from one focused
                // field (typing, dragging a number) is one step.
                let key = ui.ctx().memory(|m| m.focused()).map(|id| id.value());
                ((panel.action.take(), panel.error.take()), key)
            })
            .expect("open");
        if let Some(e) = error {
            cx.error(e);
        }
        let loaded = match action {
            None => None,
            Some(VariantAction::Edit(hopper, entry)) => {
                if !self.edit_entry_variant(hopper, entry) {
                    cx.error("this entry's game variant cannot be read");
                }
                None
            }
            Some(VariantAction::Browse) => self.browse_variant_file(),
            Some(VariantAction::Load) => {
                let path = PathBuf::from(self.variant_path.trim());
                Some(self.session.load_variant_file(&path))
            }
            Some(VariantAction::Refresh) => {
                self.session.refresh_saved_variants();
                None
            }
        };
        match loaded {
            Some(Ok(label)) => cx.info(format!("loaded {label}: pick it from an entry's variant")),
            Some(Err(e)) => cx.error(e),
            None => {}
        }
    }
}

/// What the variant controls asked for that needs more than the working
/// copy; done after the frame's edits are tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariantAction {
    /// Open entry (hopper, entry)'s variant in the variant editor.
    Edit(usize, usize),
    /// Browse for a `.h2variant` file.
    Browse,
    /// Load the typed `.h2variant` path.
    Load,
    /// Re-read the saved variants.
    Refresh,
}

/// The panel's borrowed view of the tool.
struct Panel<'a> {
    selected: &'a mut usize,
    preset: &'a mut Option<usize>,
    catalog: Option<&'a Catalog>,
    catalog_note: Option<String>,
    choices: Option<&'a Choices>,
    variant_path: &'a mut String,
    /// A file dialog is available.
    browse: bool,
    action: Option<VariantAction>,
    error: Option<String>,
}

const TYPES: [(u32, &str); 5] = [
    (1, "1 - hidden"),
    (2, "2 - playlist"),
    (3, "3 - hidden"),
    (4, "4 - playlist"),
    (5, "5 - playlist"),
];
const STATUSES: [(u32, &str); 3] = [(0, "0 - not loaded"), (1, "1 - ok"), (2, "2 - error")];

/// What a row's buttons asked for, applied after the row is drawn.
enum RowOp {
    Up(usize),
    Down(usize),
    Copy(usize),
    Remove(usize),
}

/// An entry's variant as the entries table shows it, decoded from the
/// slot it will have.
fn entry_label(base: &HopperFile, r: &EditRecord) -> String {
    let shown = model::entry_variant(base, r)
        .map(|v| variant_label(&v))
        .unwrap_or_else(|| "-".into());
    if r.picked.is_some() {
        format!("{shown} (picked)")
    } else {
        shown
    }
}

impl Panel<'_> {
    fn show(&mut self, ui: &mut egui::Ui, hoppers: &mut [EditHopper], view: &EditView<'_>) {
        egui::Panel::left("hopper_list")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.list(ui, hoppers));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let errors = model::errors(view.baseline, hoppers);
                let sel = (*self.selected).min(HOPPERS - 1);
                self.presets(ui, hoppers, view.baseline, sel);
                ui.separator();
                let h = &mut hoppers[sel];
                self.hopper(ui, h, view.base);
                if !errors.is_empty() {
                    ui.separator();
                    for e in &errors {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                }
                ui.separator();
                ui.small(
                    "To publish to a Live server, save the file and run \
                     `h2 live hopper import <file>` on the server's machine.",
                );
            });
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, hoppers: &[EditHopper]) {
        ui.heading("Playlists");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for h in hoppers {
                let name = if !h.title.is_empty() {
                    h.title.as_str()
                } else if !h.name.is_empty() {
                    h.name.as_str()
                } else {
                    "(untitled)"
                };
                let text = format!(
                    "{:>2}  {name}   t{} - {}",
                    h.index + 1,
                    h.hopper_type,
                    h.records.len()
                );
                if ui
                    .selectable_label(*self.selected == h.index, text)
                    .clicked()
                {
                    *self.selected = h.index;
                }
            }
        });
    }

    fn presets(
        &mut self,
        ui: &mut egui::Ui,
        hoppers: &mut [EditHopper],
        baseline: &[EditHopper],
        sel: usize,
    ) {
        ui.horizontal(|ui| {
            ui.label("Start from preset");
            let shown = self
                .preset
                .and_then(|i| PRESETS.get(i))
                .map(|p| format!("{} - {}", p.label, p.era))
                .unwrap_or_else(|| "Choose a historical playlist...".into());
            egui::ComboBox::from_id_salt("hopper_preset")
                .selected_text(shown)
                .width(320.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(self.preset, None, "(none)");
                    for (i, p) in PRESETS.iter().enumerate() {
                        ui.selectable_value(
                            self.preset,
                            Some(i),
                            format!("{} - {}", p.label, p.era),
                        );
                    }
                });
        });
        if let Some(note) = &self.catalog_note {
            ui.small(note);
        }
        let Some(p) = self.preset.and_then(|i| PRESETS.get(i)) else {
            return;
        };
        let r: Resolution = presets::resolve(p, self.catalog, baseline);
        ui.group(|ui| {
            ui.strong(&r.title);
            ui.label(&r.description);
            if let Some(u) = p.uncertain {
                ui.small(format!("From memory: {u}."));
            }
            for g in &r.games {
                match &g.missing {
                    None => ui.label(&g.label),
                    Some(why) => ui.weak(format!("{} - unavailable: {why}", g.label)),
                };
            }
            let label = format!("Fill playlist {} with {} entries", sel + 1, r.records.len());
            if ui
                .add_enabled(!r.records.is_empty(), egui::Button::new(label))
                .clicked()
            {
                hoppers[sel] = presets::apply(&hoppers[sel], &r);
                *self.preset = None;
            }
        });
    }

    fn hopper(&mut self, ui: &mut egui::Ui, h: &mut EditHopper, base: &HopperFile) {
        ui.heading(format!("Playlist {}", h.index + 1));
        egui::Grid::new("hopper_text")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Title");
                ui.add(
                    egui::TextEdit::singleline(&mut h.title)
                        .char_limit(MAX_TITLE)
                        .desired_width(200.0),
                );
                ui.end_row();
                ui.label("Description");
                ui.add(
                    egui::TextEdit::singleline(&mut h.description)
                        .char_limit(MAX_DESCRIPTION)
                        .desired_width(480.0),
                );
                ui.end_row();
            });
        ui.add_space(6.0);
        let mut op = None;
        let mut pick: Option<(usize, GameVariant)> = None;
        let chances = model::chances(h);
        let n = h.records.len();
        egui::Grid::new("hopper_entries")
            .striped(true)
            .num_columns(7)
            .show(ui, |ui| {
                for head in ["", "Map", "Game variant", "", "Weight", "Chance", ""] {
                    ui.strong(head);
                }
                ui.end_row();
                for (i, r) in h.records.iter_mut().enumerate() {
                    ui.label(format!("{}", i + 1));
                    self.map_picker(ui, i, r);
                    if let Some(v) = self.variant_picker(ui, i, r, base) {
                        pick = Some((i, v));
                    }
                    if ui
                        .small_button("Edit")
                        .on_hover_text("Edit in variant editor: opens a copy of this variant there")
                        .clicked()
                    {
                        self.action = Some(VariantAction::Edit(h.index, i));
                    }
                    ui.add(
                        egui::DragValue::new(&mut r.dwords[0])
                            .range(1..=MAX_WEIGHT)
                            .clamp_existing_to_range(false),
                    );
                    ui.label(format!(
                        "{:.1}%",
                        100.0 * chances.get(i).copied().unwrap_or(0.0)
                    ));
                    row_buttons(ui, i, n, &mut op);
                    ui.end_row();
                }
            });
        if ui
            .add_enabled(
                n < MAX_RECORDS,
                egui::Button::new("Add an entry (copies the last)"),
            )
            .clicked()
        {
            model::add_record(h);
        }
        if let Some((i, v)) = pick
            && let Err(e) = model::set_variant(&mut h.records[i], &v)
        {
            self.error = Some(format!("entry {}: {e}", i + 1));
        }
        self.variant_files(ui);
        egui::CollapsingHeader::new("Advanced: raw fields")
            .id_salt("hopper_advanced")
            .show(ui, |ui| advanced(ui, h, &mut op));
        if let Some(op) = op {
            let _ = match op {
                RowOp::Up(i) => model::move_record(h, i, -1),
                RowOp::Down(i) => model::move_record(h, i, 1),
                RowOp::Copy(i) => model::duplicate_record(h, i),
                RowOp::Remove(i) => model::remove_record(h, i),
            };
        }
    }

    /// The entry's variant and the picker's choices; returns the one
    /// clicked.
    fn variant_picker(
        &self,
        ui: &mut egui::Ui,
        i: usize,
        r: &EditRecord,
        base: &HopperFile,
    ) -> Option<GameVariant> {
        let current = model::entry_variant(base, r);
        let mut picked = None;
        egui::ComboBox::from_id_salt(("hopper_variant", i))
            .selected_text(entry_label(base, r))
            .width(260.0)
            .show_ui(ui, |ui| {
                let Some(c) = self.choices else {
                    ui.weak("No variants read yet.");
                    return;
                };
                for (title, list) in [
                    ("Built-in", &c.builtins),
                    ("Saved", &c.saved),
                    ("Files", &c.files),
                ] {
                    if list.is_empty() {
                        continue;
                    }
                    ui.strong(title);
                    for ch in list {
                        if let Some(v) = choice_row(ui, ch, current.as_ref()) {
                            picked = Some(v);
                        }
                    }
                }
                if c.all().next().is_none() {
                    ui.weak("No variants: open a disc, save some, or load a .h2variant file.");
                }
            });
        picked
    }

    /// Loading `.h2variant` files and refreshing the saves.
    fn variant_files(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Variant file (.h2variant)");
            ui.add(egui::TextEdit::singleline(self.variant_path).desired_width(280.0));
            if self.browse && ui.button("Browse...").clicked() {
                self.action = Some(VariantAction::Browse);
            }
            if ui
                .add_enabled(
                    !self.variant_path.trim().is_empty(),
                    egui::Button::new("Load"),
                )
                .clicked()
            {
                self.action = Some(VariantAction::Load);
            }
            if ui.button("Refresh saved variants").clicked() {
                self.action = Some(VariantAction::Refresh);
            }
        });
        if let Some(c) = self.choices {
            for n in &c.notes {
                ui.small(n);
            }
        }
    }

    fn map_picker(&self, ui: &mut egui::Ui, i: usize, r: &mut EditRecord) {
        let Some(c) = self.catalog else {
            ui.monospace(format!("map id {}", r.id));
            return;
        };
        let shown = c
            .map_by_id(r.id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("id {} (not a map in this game)", r.id));
        egui::ComboBox::from_id_salt(("hopper_map", i))
            .selected_text(shown)
            .width(180.0)
            .show_ui(ui, |ui| {
                for m in &c.maps {
                    ui.selectable_value(&mut r.id, m.id, &m.name)
                        .on_hover_text(&m.description);
                }
            });
    }
}

/// One choice in the variant menu: greyed out, with the reasons on hover,
/// when the game would reject it. Returns the variant when clicked.
fn choice_row(
    ui: &mut egui::Ui,
    ch: &Choice,
    current: Option<&GameVariant>,
) -> Option<GameVariant> {
    let on = current == Some(&ch.variant);
    let r = ui
        .add_enabled_ui(ch.usable(), |ui| ui.selectable_label(on, ch.label()))
        .inner;
    let r = if ch.usable() {
        r
    } else {
        let why: Vec<String> = ch
            .problems
            .iter()
            .map(|p| format!("+{:#x}: {}", p.offset, p.what))
            .collect();
        r.on_disabled_hover_text(format!("The game would reject it:\n{}", why.join("\n")))
    };
    r.clicked().then(|| ch.variant.clone())
}

fn row_buttons(ui: &mut egui::Ui, i: usize, n: usize, op: &mut Option<RowOp>) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(i > 0, egui::Button::new("Up").small())
            .clicked()
        {
            *op = Some(RowOp::Up(i));
        }
        if ui
            .add_enabled(i + 1 < n, egui::Button::new("Down").small())
            .clicked()
        {
            *op = Some(RowOp::Down(i));
        }
        if ui
            .add_enabled(n < MAX_RECORDS, egui::Button::new("Copy").small())
            .clicked()
        {
            *op = Some(RowOp::Copy(i));
        }
        if ui
            .add_enabled(n > 1, egui::Button::new("Remove").small())
            .clicked()
        {
            *op = Some(RowOp::Remove(i));
        }
    });
}

fn advanced(ui: &mut egui::Ui, h: &mut EditHopper, op: &mut Option<RowOp>) {
    ui.small(
        "The entry string, flags and second dword lie inside the entry's game variant, and \
         the name field overlaps a table the game validates: change them only if you know \
         the bytes.",
    );
    egui::Grid::new("hopper_raw").num_columns(2).show(ui, |ui| {
        ui.label("Name (raw, ASCII)");
        ui.add(egui::TextEdit::singleline(&mut h.name).char_limit(MAX_NAME));
        ui.end_row();
        ui.label("Type");
        choice(ui, "hopper_type", &mut h.hopper_type, &TYPES);
        ui.end_row();
        ui.label("Status");
        choice(ui, "hopper_status", &mut h.status, &STATUSES);
        ui.end_row();
        ui.label("Range fields");
        ui.horizontal(|ui| {
            for r in &mut h.ranges {
                ui.add(egui::DragValue::new(r));
            }
        });
        ui.end_row();
    });
    let n = h.records.len();
    egui::Grid::new("hopper_raw_records")
        .striped(true)
        .num_columns(7)
        .show(ui, |ui| {
            for head in ["", "map id", "string", "flags", "weight", "dword 2", ""] {
                ui.strong(head);
            }
            ui.end_row();
            for (i, r) in h.records.iter_mut().enumerate() {
                ui.label(format!("{}", i + 1));
                ui.add(egui::DragValue::new(&mut r.id));
                ui.add(
                    egui::TextEdit::singleline(&mut r.string)
                        .char_limit(MAX_STRING)
                        .desired_width(160.0),
                );
                ui.add(egui::DragValue::new(&mut r.flags).hexadecimal(8, false, true));
                ui.add(egui::DragValue::new(&mut r.dwords[0]));
                ui.add(egui::DragValue::new(&mut r.dwords[1]));
                row_buttons(ui, i, n, op);
                ui.end_row();
            }
        });
}

fn choice(ui: &mut egui::Ui, id: &str, value: &mut u32, options: &[(u32, &str)]) {
    let shown = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("{value} (out of range)"));
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (v, l) in options {
                ui.selectable_value(value, *v, *l);
            }
        });
}

#[cfg(test)]
mod tests;
