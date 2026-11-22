//! The viewport's Show popover: the one place that toggles what the viewport draws.

use dear_imgui_rs::{StyleColor, Ui};
use scenario_edit::layers::{LayerId, Preset};

use crate::app::{Action, App};
use crate::theme::{self, icon};

/// The toolbar's Show button and its popover. Returns whether the
/// button is hovered, for the status bar's hint.
pub(super) fn show_button(ui: &Ui, app: &App, actions: &mut Vec<Action>) -> bool {
    if ui.button(format!("{} Show", icon::LAYERS)) {
        ui.open_popup("viewport-layers");
    }
    let hovered = ui.is_item_hovered();
    if let Some(_popup) = ui.begin_popup("viewport-layers") {
        controls(ui, app, actions);
    }
    hovered
}

fn controls(ui: &Ui, app: &App, actions: &mut Vec<Action>) {
    let mut layers = app.layers.clone();
    let mut changed = false;
    for (i, preset) in Preset::ALL.into_iter().enumerate() {
        if i % 2 == 1 {
            ui.same_line();
        }
        if ui.button(preset.label()) {
            layers.apply(preset);
            changed = true;
        }
    }
    changed |= ui.checkbox("X-ray", &mut layers.xray);
    ui.set_item_tooltip("Draw occluded overlays at quarter strength");
    let mut previous_group = None;
    for id in LayerId::all() {
        if previous_group != Some(id.group()) {
            ui.separator();
            ui.text_disabled(id.group());
            previous_group = Some(id.group());
        }
        let [r, g, b] = theme::layer_color(id);
        let swatch = ui.push_style_color(StyleColor::Text, [r, g, b, 1.0]);
        ui.align_text_to_frame_padding();
        ui.bullet();
        swatch.pop();
        ui.same_line();
        let mut on = layers.visible(id);
        let label = format!("{} ({})###{}", id.label(), app.layer_count(id), id.id());
        if ui.checkbox(label, &mut on) {
            layers.set(id, on);
            changed = true;
        }
    }
    if changed {
        actions.push(Action::SetLayers(layers));
    }
}
