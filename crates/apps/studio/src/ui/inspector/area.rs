//! The inspector's **Area** rows for a netgame flag
//! (docs/game/gametype-markers.md, "Studio"): the shape (Radius, Box,
//! Sphere or Path), the radius or half lengths, Below and Above (a sphere
//! has none), the path's points as typed numbers, Clear,
//! and **Convert to area** for a retail hill's corners. Every edit goes
//! through [`Action::SetArea`] as one undo step (a drag is one step), and
//! an area `Area::validate` refuses goes to the status line with its
//! sentence and leaves the document as it was.
use dear_imgui_rs::Ui;
use scenario_edit::AreaFrom;
use scenario_edit::area_edit::{self, ShapeKind};
use scenario_edit::marker_view::game_markers::area::MIN_POINTS;
use scenario_edit::marker_view::game_markers::{Area, Shape};

use super::drag_fmt;
use crate::app::{Action, App};
use crate::theme;

/// The merge key of area edits to flag `index`: a drag is one undo step.
pub(crate) fn merge_key(index: usize) -> u64 {
    0xa7ea_0000_0000 | index as u64
}

/// Stores `next` for flag `index` when it validates, else says why not.
fn store(app: &mut App, index: usize, next: Result<Area, String>, merge: bool) {
    match next {
        Ok(area) => app.dispatch(Action::SetArea {
            index,
            area: Some(area),
            merge: merge.then(|| merge_key(index)),
        }),
        Err(why) => app.error(why),
    }
}

/// The rows, for the one selected flag `index`.
pub(super) fn rows(ui: &Ui, app: &mut App, index: usize) {
    let corners = app.session.hill_corners(index);
    if !corners.is_empty() {
        theme::section(ui, "Area");
        ui.text_disabled(format!(
            "One of {} corners of a hill: it has no area of its own.",
            corners.len()
        ));
        if ui.button("Convert to area") {
            app.dispatch(Action::ConvertHill(index));
        }
        ui.set_item_tooltip(
            "Replaces the corners with one flag at their middle whose area is their hull. One undo step.",
        );
        return;
    }
    let Some((_, area, from)) = app.session.editable_area(index) else {
        return;
    };
    theme::section(ui, "Area");
    if from == AreaFrom::Default {
        ui.text_disabled("Not shaped: the default cylinder. An edit stores an area.");
    }
    let _id = ui.push_id(index);

    // Shape.
    let kind = ShapeKind::of(&area.shape);
    ui.text("Shape");
    for k in ShapeKind::ALL {
        ui.same_line();
        let label = if k == kind {
            format!("[{}]", k.name())
        } else {
            k.name().to_owned()
        };
        if ui.small_button(label) && k != kind {
            let yaw = app.session.flag_yaw(index);
            let next = area_edit::with_shape(&area, k, yaw).map_err(|e| e.to_string());
            store(app, index, next, false);
        }
    }

    let mut next = area.clone();
    let mut changed = false;
    let mut done = false;
    match &mut next.shape {
        Shape::Radius(r) | Shape::Sphere(r) => {
            changed |= drag_fmt(ui, "Radius".to_owned(), 0.05, "%.2f", r);
            done |= ui.is_item_deactivated_after_edit();
        }
        Shape::Box { half } => {
            for (i, name) in ["Half x", "Half y"].into_iter().enumerate() {
                changed |= drag_fmt(ui, name.to_owned(), 0.05, "%.2f", &mut half[i]);
                done |= ui.is_item_deactivated_after_edit();
            }
        }
        Shape::Path(_) => {}
    }
    // A ball has no height: its radius is all of it.
    if !matches!(next.shape, Shape::Sphere(_)) {
        changed |= drag_fmt(ui, "Below".to_owned(), 0.05, "%.2f", &mut next.below);
        done |= ui.is_item_deactivated_after_edit();
        changed |= drag_fmt(ui, "Above".to_owned(), 0.05, "%.2f", &mut next.above);
        done |= ui.is_item_deactivated_after_edit();
    }

    let mut remove = None;
    if let Shape::Path(points) = &mut next.shape {
        let n = points.len();
        for (i, p) in points.iter_mut().enumerate() {
            let _row = ui.push_id(i);
            changed |= drag_fmt(ui, format!("Point {}  x", i + 1), 0.05, "%.2f", &mut p[0]);
            done |= ui.is_item_deactivated_after_edit();
            ui.same_line();
            changed |= drag_fmt(ui, "y".to_owned(), 0.05, "%.2f", &mut p[1]);
            done |= ui.is_item_deactivated_after_edit();
            ui.same_line();
            if ui.small_button("Remove") {
                remove = Some(i);
            }
            if n <= MIN_POINTS {
                ui.set_item_tooltip("A path keeps at least 3 points.");
            }
        }
    }
    if let Some(i) = remove {
        let r = area_edit::remove_point(&area, i).map_err(|e| e.to_string());
        store(app, index, r, false);
        return;
    }
    if changed {
        let valid = next.validate().map(|()| next).map_err(|e| e.to_string());
        store(app, index, valid, true);
    }
    if from == AreaFrom::Stored && ui.button("Clear area") {
        app.dispatch(Action::SetArea {
            index,
            area: None,
            merge: None,
        });
    }
    if done {
        app.dispatch(Action::SealEdit);
    }
}
