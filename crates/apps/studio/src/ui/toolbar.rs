//! Map viewport tools. The labels behind the glyphs are stable script targets.

use dear_imgui_rs::{SliderFlags, StyleColor, StyleVar, Ui};

use super::layers;
use crate::app::{Action, App};
use crate::camera::{MAX_SPEED, MIN_SPEED};
use crate::theme::{self, icon};
use crate::viewport::{ANGLE_STEPS, GRID_STEPS, GizmoOp, ViewportView};

pub(super) fn draw(
    ui: &Ui,
    app: &App,
    viewport: &mut ViewportView,
    actions: &mut Vec<Action>,
    hint: &mut Option<String>,
) {
    let _padding = ui.push_style_var(StyleVar::FramePadding([6.0, 4.0]));
    let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
    ui.dummy([0.0, 2.0]);
    ui.group(|| {
        for (index, (op, glyph, name)) in [
            (GizmoOp::Translate, icon::MOVE, "Move (W)"),
            (GizmoOp::Rotate, icon::ROTATE, "Rotate (E)"),
            (GizmoOp::Scale, icon::MAXIMIZE, "Scale (R)"),
        ]
        .into_iter()
        .enumerate()
        {
            if index != 0 {
                ui.same_line();
            }
            if button(ui, hint, glyph, name, viewport.op == op) {
                viewport.op = op;
            }
        }
        ui.same_line();
        let axes = if viewport.local {
            "Local axes (X)"
        } else {
            "World axes (X)"
        };
        let glyph = if viewport.local {
            icon::BOX
        } else {
            icon::CROSSHAIR
        };
        if button(ui, hint, glyph, axes, viewport.local) {
            viewport.local = !viewport.local;
        }
    });

    next_group(ui, right, 207.0);
    ui.group(|| {
        let snap = &mut viewport.snap;
        if button(ui, hint, "# ", "Toggle grid snapping", snap.grid) {
            snap.grid = !snap.grid;
        }
        ui.same_line();
        if step(ui, hint, "Grid step", "", &mut snap.grid_step, &GRID_STEPS) {
            snap.grid = true;
        }
        ui.same_line();
        if button(ui, hint, "∠", "Toggle angle snapping", snap.angle) {
            snap.angle = !snap.angle;
        }
        ui.same_line();
        if step(
            ui,
            hint,
            "Angle step",
            "°",
            &mut snap.angle_step,
            &ANGLE_STEPS,
        ) {
            snap.angle = true;
        }
    });

    next_group(ui, right, 155.0);
    ui.group(|| {
        if button(ui, hint, icon::MAXIMIZE, "Frame map (Home)", false) {
            actions.push(Action::FrameAll);
        }
        ui.same_line();
        if button(ui, hint, icon::FOCUS, "Frame selection (F)", false) {
            actions.push(Action::FocusSelected);
        }
        ui.same_line();
        ui.set_next_item_width(76.0);
        let mut speed = app.fly_speed();
        if ui
            .slider_config("##Fly speed", MIN_SPEED, MAX_SPEED)
            .display_format(dear_imgui_rs::NumericFormat::new("%.1f u/s").expect("a valid float format"))
            .flags(SliderFlags::LOGARITHMIC | SliderFlags::ALWAYS_CLAMP)
            .build(&mut speed)
        {
            actions.push(Action::SetFlySpeed(speed));
        }
        describe(ui, hint, "Fly speed (wheel while flying)");
    });

    next_group(ui, right, 76.0);
    if layers::show_button(ui, app, actions) {
        *hint = Some("Choose what the viewport shows".to_owned());
    }
}

fn next_group(ui: &Ui, right: f32, width: f32) {
    if ui.item_rect_max()[0] + width + 14.0 <= right {
        ui.same_line_with_spacing(0.0, 14.0);
    }
}

fn button(ui: &Ui, hint: &mut Option<String>, glyph: &str, name: &str, active: bool) -> bool {
    let tone = active.then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
    let clicked = ui.button(format!("{glyph}##{name}"));
    drop(tone);
    describe(ui, hint, name);
    clicked
}

fn step(
    ui: &Ui,
    hint: &mut Option<String>,
    name: &str,
    suffix: &str,
    value: &mut f32,
    steps: &[f32],
) -> bool {
    ui.set_next_item_width(60.0);
    let combo = ui.begin_combo(format!("##{name}"), format!("{value}{suffix}"));
    let mut changed = false;
    if let Some(_combo) = combo {
        for &s in steps {
            if ui
                .selectable_config(format!("{s}{suffix}"))
                .selected(s == *value)
                .build()
            {
                *value = s;
                changed = true;
            }
        }
    }
    describe(ui, hint, name);
    changed
}

fn describe(ui: &Ui, hint: &mut Option<String>, text: &str) {
    if ui.is_item_hovered() {
        *hint = Some(text.to_owned());
    }
    ui.set_item_tooltip(text);
}
