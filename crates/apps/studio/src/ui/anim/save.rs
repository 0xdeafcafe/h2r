//! The Animation window's package save dialog.

use dear_imgui_rs::Ui;

use crate::App;
use crate::app::anim::AnimationPlacement;
use crate::app::assets::{CopyRefusal, read_set};
use crate::theme;

const POPUP: &str = "Save animation to package###anim-save";

#[derive(Default)]
pub(super) struct State {
    packages: Vec<(String, String)>,
    package: Option<String>,
    doc: Option<u64>,
    mode: String,
    class: String,
    weapon_type: String,
    action: String,
    error: Option<String>,
    saved: Option<String>,
}

impl State {
    pub(super) fn draw(&mut self, ui: &Ui, app: &mut App, doc: u64, replace: bool) {
        if self.doc != Some(doc) {
            self.doc = Some(doc);
            self.mode = "any".to_owned();
            self.class = "any".to_owned();
            self.weapon_type = "any".to_owned();
            self.action.clear();
            self.error = None;
            self.saved = None;
        }
        if ui.button("Save to package…##anim-save-open") {
            self.packages = read_set(&app.data_root())
                .listed
                .into_iter()
                .filter(|row| row.dir.is_some())
                .map(|row| (row.id, row.name))
                .collect();
            if !self
                .packages
                .iter()
                .any(|(id, _)| Some(id) == self.package.as_ref())
            {
                self.package = self.packages.first().map(|(id, _)| id.clone());
            }
            self.error = None;
            ui.open_popup(POPUP);
        }
        if let Some(path) = &self.saved {
            ui.text_colored(theme::OK, path);
        }
        let Some(_dialog) = ui.begin_modal_popup_config(POPUP).begin() else {
            return;
        };
        if self.packages.is_empty() {
            ui.text("No local packages");
        } else {
            let selected = self
                .package
                .as_ref()
                .and_then(|id| self.packages.iter().find(|(entry, _)| entry == id))
                .map(|(id, name)| format!("{name} ({id})"))
                .unwrap_or_default();
            if let Some(_combo) = ui.begin_combo("Package##anim-save-package", selected) {
                for (id, name) in &self.packages {
                    if ui
                        .selectable_config(format!("{name} ({id})##anim-package-{id}"))
                        .selected(self.package.as_ref() == Some(id))
                        .build()
                    {
                        self.package = Some(id.clone());
                    }
                }
            }
        }
        if replace {
            ui.text_disabled("Replace this clip");
        } else {
            for (label, value) in [
                ("Mode##anim-save-mode", &mut self.mode),
                ("Class##anim-save-class", &mut self.class),
                ("Weapon type##anim-save-type", &mut self.weapon_type),
                ("Action##anim-save-action", &mut self.action),
            ] {
                ui.input_text(label, value).build();
            }
        }
        if let Some(error) = &self.error {
            ui.text_colored(theme::WARN, error);
        }
        let disabled = ui.begin_disabled_with_cond(
            self.package.is_none() || (!replace && self.action.trim().is_empty()),
        );
        if ui.button("Save##anim-save-submit")
            && let Some(package) = &self.package
        {
            let placement = if replace {
                AnimationPlacement::default()
            } else {
                AnimationPlacement {
                    mode: Some(&self.mode),
                    class: Some(&self.class),
                    weapon_type: Some(&self.weapon_type),
                    action: Some(&self.action),
                }
            };
            match app.anim_save(doc, package, placement) {
                Ok(saved) => {
                    self.saved = Some(format!("Saved to {}", saved.package));
                    ui.close_current_popup();
                }
                Err(CopyRefusal::Params(e) | CopyRefusal::Refused(e) | CopyRefusal::Failed(e)) => {
                    self.error = Some(e);
                }
            }
        }
        drop(disabled);
        ui.same_line();
        if ui.button("Cancel##anim-save-cancel") {
            ui.close_current_popup();
        }
    }
}
