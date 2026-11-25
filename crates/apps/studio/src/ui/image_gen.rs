//! The Generate window (docs/editor/editor-guide.md, "Generating an
//! image"): a prompt, the references the user picks (the other images of
//! the slot's bitmap, and PNG files), then the job's state and Apply,
//! Accept, Discard and Regenerate. The Textures tool's inspector, a HUD
//! bitmap widget and a UI Screens picture open it on their slot. State
//! lives in [`crate::app::image_gen::Panel`]; everything goes back as
//! [`ImageAction`]s, the commands `image.*` runs.
//!
//! The window shows no picture of its own: Apply puts the fitted picture
//! in its slot, where the Textures canvas, the UI Screens picture and a
//! paired game show it.

use std::path::PathBuf;

use dear_imgui_rs::{Condition, Ui};

use crate::app::image_gen::{GenInput, ImageAction, RefInput};
use crate::app::{Action, App};
use crate::imagegen::Alpha;
use crate::theme::{self, icon};

const ALPHAS: [Alpha; 4] = [Alpha::Auto, Alpha::Image, Alpha::Luma, Alpha::Opaque];

fn alpha_label(a: Alpha) -> &'static str {
    match a {
        Alpha::Auto => "Auto",
        Alpha::Image => "The picture's own",
        Alpha::Luma => "From brightness",
        Alpha::Opaque => "Opaque",
    }
}

pub fn window(ui: &Ui, app: &mut App, dialogs: bool, actions: &mut Vec<Action>) {
    if !app.image_panel.open {
        return;
    }
    app.imagegen.poll();
    let mut open = true;
    ui.window("Generate image###image-gen")
        .size([460.0, 520.0], Condition::FirstUseEver)
        .opened(&mut open)
        .build(|| body(ui, app, dialogs, actions));
    if !open {
        app.image_panel.open = false;
    }
}

fn body(ui: &Ui, app: &mut App, dialogs: bool, actions: &mut Vec<Action>) {
    let Some((bitmap, image)) = app.image_panel.slot.clone() else {
        ui.text_disabled("Pick an image: Generate in the Textures tool, a HUD bitmap widget or a UI screen's picture.");
        return;
    };
    ui.text(format!("{} {bitmap}", icon::FILE));
    let slot = app.image_slot(&bitmap, image);
    let images = app
        .resolve_bitmap(&bitmap)
        .ok()
        .and_then(|t| app.textures.open.get(&t))
        .map_or(0, |o| o.images.len());
    ui.set_next_item_width(120.0);
    let mut n = image as i32;
    if ui.input_int("Image##gen-image", &mut n) {
        let n = n.clamp(0, images.saturating_sub(1) as i32) as usize;
        if n != image {
            app.image_panel.slot = Some((bitmap.clone(), n));
            app.image_panel.picked.retain(|i| *i != n);
        }
    }
    ui.same_line();
    if ui.button(format!("{} New image##gen-new-slot", icon::PLUS)) {
        actions.push(Action::Image(ImageAction::NewSlot {
            bitmap: bitmap.clone(),
            like: image,
        }));
    }
    ui.set_item_tooltip(
        "Add an image to this bitmap, the size and format of this one, and generate into it",
    );
    match &slot {
        Ok(s) => ui.text_disabled(format!(
            "{} by {}, {:?}: the picture is fitted to this",
            s.width, s.height, s.format
        )),
        Err(e) => {
            ui.text_colored(theme::ERROR, format!("{} {e}", icon::ALERT));
        }
    }
    theme::section(ui, "Prompt");
    ui.input_text_multiline("##gen-prompt", &mut app.image_panel.prompt, [-1.0, 80.0])
        .build();
    theme::section(ui, "References");
    ui.text_wrapped(
        "Pick the images whose style the new one should match, such as the icons beside it. They are your choice: they go to the generator as they are.",
    );
    for i in (0..images).filter(|i| *i != image) {
        let mut on = app.image_panel.picked.contains(&i);
        if ui.checkbox(format!("Image {i} of this bitmap##gen-ref-{i}"), &mut on) {
            if on {
                app.image_panel.picked.push(i);
            } else {
                app.image_panel.picked.retain(|x| *x != i);
            }
        }
    }
    let mut drop_file = None;
    for (i, f) in app.image_panel.files.iter().enumerate() {
        if ui.small_button(format!("{}##gen-file-{i}", icon::CLOSE)) {
            drop_file = Some(i);
        }
        ui.same_line();
        ui.text(f.display().to_string());
    }
    if let Some(i) = drop_file {
        app.image_panel.files.remove(i);
    }
    if ui.button(format!("{} Add PNG...##gen-add", icon::PLUS))
        && let Some(path) = super::textures::import_dialog(dialogs)
    {
        app.image_panel.files.push(path);
    }
    ui.set_next_item_width(200.0);
    if let Some(_c) = ui.begin_combo("Alpha##gen-alpha", alpha_label(app.image_panel.alpha)) {
        for a in ALPHAS {
            if ui
                .selectable_config(alpha_label(a))
                .selected(a == app.image_panel.alpha)
                .build()
            {
                app.image_panel.alpha = a;
            }
        }
    }
    ui.separator();
    let busy = app.imagegen.busy();
    let p = &app.image_panel;
    let mut references: Vec<RefInput> = p
        .picked
        .iter()
        .map(|i| RefInput::Map {
            bitmap: bitmap.clone(),
            image: *i,
        })
        .collect();
    references.extend(p.files.iter().cloned().map(RefInput::File));
    let input = GenInput {
        prompt: p.prompt.clone(),
        references,
        slot: Some((bitmap.clone(), image)),
        size: None,
        alpha: p.alpha,
        out: None,
        from: p.job,
    };
    let can = !busy && slot.is_ok() && !p.prompt.trim().is_empty();
    let d = ui.begin_disabled_with_cond(!can);
    let label = if p.job.is_some() { "Regenerate" } else { "Generate" };
    if ui.button(format!("{} {label}##gen-go", icon::ZAP)) {
        actions.push(Action::Image(ImageAction::Generate(Box::new(input))));
    }
    d.end();
    ui.same_line();
    ui.text_disabled(format!("with {}", app.imagegen.source_name()));
    let Some(id) = app.image_panel.job else {
        return;
    };
    let Some(job) = app.imagegen.job(id) else {
        return;
    };
    let report = job.report();
    theme::section(ui, format!("Image {id}"));
    match job.state_name() {
        "running" => ui.text(format!(
            "Generating... {} s",
            report["elapsed_ms"].as_u64().unwrap_or(0) / 1000
        )),
        "failed" => {
            ui.text_colored(theme::ERROR, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(report["why"].as_str().unwrap_or("failed"));
        }
        _ => {
            ui.text(format!(
                "Made {} by {}, fitted to {} by {} ({} alpha)",
                report["raw"]["width"], report["raw"]["height"], report["fitted"]["width"], report["fitted"]["height"],
                report["fitted"]["alpha"].as_str().unwrap_or("")
            ));
            ui.text_disabled(report["fitted"]["path"].as_str().unwrap_or(""));
            let applied = job.applied.is_some();
            let accepted = job.accepted.is_some();
            if !applied {
                if ui.button(format!("{} Apply (preview in place)##gen-apply", icon::EYE)) {
                    actions.push(Action::Image(ImageAction::Apply(id)));
                }
                ui.same_line();
            }
            if !accepted && ui.button(format!("{} Accept##gen-accept", icon::CHECK)) {
                actions.push(Action::Image(ImageAction::Accept { id, save: None }));
            }
            if !accepted {
                ui.same_line();
            }
            if ui.button(format!("{} Accept and save art to...##gen-save", icon::SAVE))
                && let Some(dir) = save_dialog(dialogs)
            {
                actions.push(Action::Image(ImageAction::Accept { id, save: Some(dir) }));
            }
            if applied {
                if ui.button(format!("{} Discard##gen-discard", icon::UNDO)) {
                    actions.push(Action::Image(ImageAction::Discard(id)));
                }
                ui.text_disabled(
                    "In its slot now: the Textures canvas, the UI Screens picture and a paired game show it. Accept keeps it; the map's save writes it.",
                );
            }
            if accepted {
                ui.text_colored(theme::OK, format!("{} Accepted: saves with the map", icon::CHECK));
            }
        }
    }
}

/// A directory to save the art and its provenance in.
fn save_dialog(dialogs: bool) -> Option<PathBuf> {
    if !dialogs {
        return None;
    }
    rfd::FileDialog::new()
        .set_title("Save the art and its provenance")
        .pick_folder()
}
