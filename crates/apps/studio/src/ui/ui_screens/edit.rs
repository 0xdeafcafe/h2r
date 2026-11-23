//! Editing on the UI Screens picture (docs/game/ui-builder.md "Editing in
//! the editor"): click to select, drag to move, the handles to resize,
//! a double-click on a text to retype it in place, arrow keys to nudge;
//! and the inspector's **Selected** section and widget list. Every edit
//! goes back as [`Action::UiEdit`], so it is the agent API's edit too.

use dear_imgui_rs::{Key, MouseButton, Ui};

use boot::ui_kit::view::{Kind, View, Widget};

use crate::app::ui_screens::edit::{EditAction, Sink};
use crate::app::image_gen::ImageAction;
use crate::app::{Action, App};
use crate::theme::{self, Fonts, icon};

/// Snapping reaches this far, in UI units.
const SNAP: i32 = 4;
/// A handle's half size, in points.
const HANDLE: f32 = 4.0;

/// Which part of the selection a drag holds: the body moves it, an edge
/// or corner handle moves those edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grip {
    top: bool,
    left: bool,
    bottom: bool,
    right: bool,
}

impl Grip {
    const BODY: Grip = Grip {
        top: true,
        left: true,
        bottom: true,
        right: true,
    };
    fn is_body(self) -> bool {
        self == Grip::BODY
    }
}

/// The eight handles, in UI units relative to the rect, as grips.
const HANDLES: [Grip; 8] = [
    Grip { top: true, left: true, bottom: false, right: false },
    Grip { top: true, left: false, bottom: false, right: false },
    Grip { top: true, left: false, bottom: false, right: true },
    Grip { top: false, left: false, bottom: false, right: true },
    Grip { top: false, left: false, bottom: true, right: true },
    Grip { top: false, left: false, bottom: true, right: false },
    Grip { top: false, left: true, bottom: true, right: false },
    Grip { top: false, left: true, bottom: false, right: false },
];

struct Drag {
    id: String,
    grip: Grip,
    /// The mouse at the start, in points.
    from: [f32; 2],
    /// The rect at the start.
    start: [i16; 4],
    moved: bool,
}

struct Typing {
    id: String,
    buf: String,
    /// Focus the input on its first frame.
    fresh: bool,
}

/// The editing's UI-only state.
#[derive(Default)]
pub struct EditUi {
    drag: Option<Drag>,
    typing: Option<Typing>,
    /// The inspector's text field, for the widget it was filled from.
    text: Option<(String, String)>,
    /// The widget whose row of the widget list is under the mouse: lit in
    /// the picture.
    pub list_hot: Option<String>,
}

/// Whether the sink takes a rect for `w` with a new size (the handles).
fn resizable(sink: Sink, w: &Widget) -> bool {
    match sink {
        Sink::Ours => !w.id.starts_with("row."),
        Sink::Retail => w.id.starts_with("text.") || w.id.starts_with("button."),
    }
}

/// A handle's centre in UI pixels (top left origin).
fn handle_at(g: Grip, px: [i32; 4]) -> [f32; 2] {
    let [x, y, w, h] = px.map(|v| v as f32);
    let hx = if g.left {
        x
    } else if g.right {
        x + w
    } else {
        x + w / 2.0
    };
    let hy = if g.top {
        y
    } else if g.bottom {
        y + h
    } else {
        y + h / 2.0
    };
    [hx, hy]
}

/// `start` with the grip's edges moved by `dx`, `dy` units (Y up).
fn moved(start: [i16; 4], g: Grip, dx: i32, dy: i32) -> [i32; 4] {
    let [t, l, b, r] = start.map(i32::from);
    let mut out = [t, l, b, r];
    if g.is_body() {
        return [t + dy, l + dx, b + dy, r + dx];
    }
    if g.top {
        out[0] = (t + dy).max(b);
    }
    if g.bottom {
        out[2] = (b + dy).min(t);
    }
    if g.left {
        out[1] = (l + dx).min(r);
    }
    if g.right {
        out[3] = (r + dx).max(l);
    }
    out
}

/// The nearest of `targets` to any of `ours` within [`SNAP`]: the shift
/// that lands on it.
fn snap_axis(ours: &[i32], targets: &[i32]) -> i32 {
    let mut best: Option<i32> = None;
    for &o in ours {
        for &t in targets {
            let d = t - o;
            if d.abs() <= SNAP && best.is_none_or(|b| d.abs() < b.abs()) {
                best = Some(d);
            }
        }
    }
    best.unwrap_or(0)
}

/// `r` snapped to the other widgets' edges and centres (and the screen's
/// centre lines): a body moves whole, a handle moves its own edges.
fn snapped(r: [i32; 4], g: Grip, id: &str, view: &View) -> [i32; 4] {
    let mut xs = vec![0];
    let mut ys = vec![0];
    for w in view.widgets.iter().filter(|w| w.id != id && w.alpha != 0) {
        let [t, l, b, rr] = w.rect.map(i32::from);
        xs.extend([l, rr, (l + rr) / 2]);
        ys.extend([t, b, (t + b) / 2]);
    }
    let [t, l, b, rr] = r;
    if g.is_body() {
        let dx = snap_axis(&[l, rr, (l + rr) / 2], &xs);
        let dy = snap_axis(&[t, b, (t + b) / 2], &ys);
        return [t + dy, l + dx, b + dy, rr + dx];
    }
    let mut out = r;
    if g.left {
        out[1] += snap_axis(&[l], &xs);
    }
    if g.right {
        out[3] += snap_axis(&[rr], &xs);
    }
    if g.top {
        out[0] += snap_axis(&[t], &ys);
    }
    if g.bottom {
        out[2] += snap_axis(&[b], &ys);
    }
    out
}

fn clamp16(r: [i32; 4]) -> [i16; 4] {
    r.map(|v| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16)
}

/// The topmost visible widget at UI pixel `px`.
fn hit(view: &View, px: [f32; 2]) -> Option<String> {
    view.widgets
        .iter()
        .rev()
        .filter(|w| w.alpha != 0)
        .find(|w| {
            let [x, y, wd, h] = w.pixels().map(|v| v as f32);
            px[0] >= x && px[0] < x + wd && px[1] >= y && px[1] < y + h
        })
        .map(|w| w.id.clone())
}

/// The picture's editing, after it is drawn: `origin` and `zoom` place it,
/// `hovered` says the mouse is over it.
pub fn canvas(
    ui: &Ui,
    app: &mut App,
    st: &mut EditUi,
    origin: [f32; 2],
    zoom: f32,
    hovered: bool,
    actions: &mut Vec<Action>,
) {
    let Some(sink) = app.ui_screens.pick.as_ref().map(Sink::of) else {
        return;
    };
    let Some(view) = app.ui_screens.edit_view().cloned() else {
        return;
    };
    let selected = app.ui_screens.edit.selected.clone();
    let at = |px: [f32; 2]| [origin[0] + px[0] * zoom, origin[1] + px[1] * zoom];
    let dl = ui.get_window_draw_list();
    let m = ui.io().mouse_pos();
    let px = [(m[0] - origin[0]) / zoom, (m[1] - origin[1]) / zoom];
    let sel = selected.as_deref().and_then(|id| view.widget(id));

    // The list's hovered row, and the selection with its handles.
    if let Some(w) = st.list_hot.as_deref().and_then(|id| view.widget(id)) {
        let [x, y, wd, h] = w.pixels().map(|v| v as f32);
        dl.add_rect(at([x, y]), at([x + wd, y + h]), theme::WIDGET_FOCUS)
            .thickness(3.0)
            .build();
    }
    let handles: Vec<(Grip, [f32; 2])> = match sel {
        Some(w) if resizable(sink, w) => HANDLES
            .iter()
            .map(|g| (*g, at(handle_at(*g, w.pixels()))))
            .collect(),
        _ => Vec::new(),
    };
    if let Some(w) = sel {
        let [x, y, wd, h] = w.pixels().map(|v| v as f32);
        dl.add_rect(at([x, y]), at([x + wd, y + h]), theme::ACCENT)
            .thickness(2.0)
            .build();
        for (_, c) in &handles {
            dl.add_rect(
                [c[0] - HANDLE, c[1] - HANDLE],
                [c[0] + HANDLE, c[1] + HANDLE],
                theme::ACCENT,
            )
            .filled(true)
            .build();
        }
    }

    // Press: a handle of the selection, else the widget under the mouse.
    if hovered && st.typing.is_none() && ui.is_mouse_clicked(MouseButton::Left) {
        actions.push(Action::UiEdit(EditAction::Seal));
        let grip = handles
            .iter()
            .find(|(_, c)| (m[0] - c[0]).abs() <= HANDLE + 1.0 && (m[1] - c[1]).abs() <= HANDLE + 1.0)
            .map(|(g, _)| *g);
        match (grip, sel) {
            (Some(grip), Some(w)) => {
                st.drag = Some(Drag {
                    id: w.id.clone(),
                    grip,
                    from: m,
                    start: w.rect,
                    moved: false,
                });
            }
            _ => {
                let id = hit(&view, px);
                if id != selected {
                    actions.push(Action::UiEdit(EditAction::Select(id.clone())));
                }
                st.drag = id.and_then(|id| {
                    let w = view.widget(&id)?;
                    Some(Drag {
                        id,
                        grip: Grip::BODY,
                        from: m,
                        start: w.rect,
                        moved: false,
                    })
                });
            }
        }
    }
    // Double-click a text: type over it.
    if hovered && ui.is_mouse_double_clicked(MouseButton::Left) {
        if let Some(w) = hit(&view, px).and_then(|id| view.widget(&id).cloned())
            && let Kind::Text { text, .. } = &w.kind
        {
            st.drag = None;
            st.typing = Some(Typing {
                id: w.id.clone(),
                buf: text.clone(),
                fresh: true,
            });
        }
    }
    // Drag: the rect from its start, snapped unless Shift is held.
    if let Some(d) = &mut st.drag {
        if ui.is_mouse_down(MouseButton::Left) {
            let dx = ((m[0] - d.from[0]) / zoom).round() as i32;
            let dy = (-(m[1] - d.from[1]) / zoom).round() as i32;
            if dx != 0 || dy != 0 || d.moved {
                let mut r = moved(d.start, d.grip, dx, dy);
                if !ui.io().key_shift() {
                    r = snapped(r, d.grip, &d.id, &view);
                }
                let r = clamp16(r);
                let now = view.widget(&d.id).map(|w| w.rect);
                if now != Some(r) {
                    actions.push(Action::UiEdit(EditAction::SetRect {
                        id: d.id.clone(),
                        rect: r,
                        merge: d.moved,
                    }));
                    d.moved = true;
                }
            }
        } else {
            if d.moved {
                actions.push(Action::UiEdit(EditAction::Seal));
            }
            st.drag = None;
        }
    }
    // Arrow keys nudge the selection: 1 unit, 10 with Shift.
    if st.typing.is_none()
        && let Some(id) = selected.clone()
        && ui.is_window_focused()
        && !ui.io().want_text_input()
    {
        let step = if ui.io().key_shift() { 10 } else { 1 };
        let nudge = [
            (Key::LeftArrow, -step, 0),
            (Key::RightArrow, step, 0),
            (Key::UpArrow, 0, step),
            (Key::DownArrow, 0, -step),
        ];
        for (key, dx, dy) in nudge {
            if ui.is_key_pressed_with_repeat(key, true) {
                actions.push(Action::UiEdit(EditAction::Move {
                    id: id.clone(),
                    dx,
                    dy,
                    merge: true,
                }));
            }
        }
        if ui.is_key_pressed(Key::Escape) {
            actions.push(Action::UiEdit(EditAction::Select(None)));
        }
    }
    // The text being typed, in an input over its rect.
    if let Some(t) = &mut st.typing {
        let Some(w) = view.widget(&t.id) else {
            st.typing = None;
            return;
        };
        let [x, y, wd, _] = w.pixels().map(|v| v as f32);
        ui.set_cursor_screen_pos(at([x, y]));
        ui.set_next_item_width((wd * zoom).max(80.0));
        if t.fresh {
            ui.set_keyboard_focus_here();
        }
        let done = ui
            .input_text("##ui-inline-text", &mut t.buf)
            .enter_returns_true(true)
            .auto_select_all(true)
            .build();
        let fresh = std::mem::replace(&mut t.fresh, false);
        if done {
            actions.push(Action::UiEdit(EditAction::SetText {
                id: t.id.clone(),
                text: t.buf.clone(),
            }));
            st.typing = None;
        } else if ui.is_key_pressed(Key::Escape) || (!fresh && ui.is_item_deactivated()) {
            st.typing = None;
        }
    }
}

/// The inspector's **Selected** section: the widget's id and kind, its
/// rect as numbers, its text, what can't be edited read-only, and the
/// copy, revert and save buttons.
pub fn selected(
    ui: &Ui,
    app: &mut App,
    st: &mut EditUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let heading = |t: &str| {
        let f = ui.push_font(fonts.bold);
        ui.text(t);
        f.pop();
    };
    heading("Selected");
    let Some(sink) = app.ui_screens.pick.as_ref().map(Sink::of) else {
        ui.text_disabled("Pick a screen.");
        return;
    };
    let id = app.ui_screens.edit.selected.clone();
    let w = id.as_deref().and_then(|id| app.ui_screens.edit_widget(id).ok());
    match &w {
        None => ui.text_disabled("Click a widget in the picture to select it."),
        Some(w) => {
            ui.text(format!("{}  ({:?}, alpha {})", w.id, w.layer, w.alpha));
            let mut r = w.rect.map(i32::from);
            let before = r;
            for (i, name) in ["Top", "Left", "Bottom", "Right"].iter().enumerate() {
                ui.set_next_item_width(110.0);
                ui.input_int(format!("{name}##ui-sel-{name}"), &mut r[i]);
                if i % 2 == 0 {
                    ui.same_line();
                }
            }
            if r != before {
                actions.push(Action::UiEdit(EditAction::SetRect {
                    id: w.id.clone(),
                    rect: clamp16(r),
                    merge: true,
                }));
            }
            if let Kind::Text {
                text,
                font,
                justify,
                colour,
            } = &w.kind
            {
                if st.text.as_ref().is_none_or(|(for_id, _)| *for_id != w.id) {
                    st.text = Some((w.id.clone(), text.clone()));
                }
                let buf = &mut st.text.as_mut().expect("just set").1;
                ui.set_next_item_width(-1.0);
                if ui
                    .input_text("##ui-sel-text", buf)
                    .enter_returns_true(true)
                    .build()
                {
                    actions.push(Action::UiEdit(EditAction::SetText {
                        id: w.id.clone(),
                        text: buf.clone(),
                    }));
                }
                ui.text_disabled(format!(
                    "font {font}, {justify:?}, colour {colour:?} (from the component)"
                ));
            }
            if ui.button(format!("{} Copy JSON", icon::COPY))
                && let Some(json) = app.ui_screens.edit_widget_json(&w.id)
            {
                let text = serde_json::to_string_pretty(&json).unwrap_or_default();
                if let Ok(text) = std::ffi::CString::new(text) {
                    // SAFETY: a frame is being built, so ImGui has a
                    // current context; the string outlives the call.
                    unsafe { dear_imgui_rs::sys::igSetClipboardText(text.as_ptr()) };
                }
            }
            ui.same_line();
            if ui.button(format!("{} Revert widget", icon::UNDO)) {
                actions.push(Action::UiEdit(EditAction::Revert(Some(w.id.clone()))));
            }
            // A picture: the Generate window on the bitmap image it draws.
            if matches!(w.kind, Kind::Art { .. })
                && let Some((bitmap, image)) = app.ui_screens.widget_bitmap(&w.id)
            {
                if ui.button(format!("{} Generate image...##ui-sel-generate", icon::ZAP)) {
                    actions.push(Action::Image(ImageAction::Open { bitmap: bitmap.clone(), image }));
                }
                ui.set_item_tooltip(format!(
                    "{bitmap} image {image}: make a new picture for it (the bitmap opens from the map open in Studio)"
                ));
            }
            if sink == Sink::Retail {
                // Tag edits past a rect (app::ui_screens::edit::retail_tag);
                // a refusal names the owner in the log.
                if ui.button("Hide##ui-sel-hide") {
                    actions.push(Action::UiEdit(EditAction::Hide { id: w.id.clone() }));
                }
                ui.same_line();
                if ui.button("Duplicate##ui-sel-duplicate") {
                    actions.push(Action::UiEdit(EditAction::Duplicate { id: w.id.clone() }));
                }
            }
        }
    }
    if ui.button(format!("{} Revert screen", icon::UNDO)) {
        actions.push(Action::UiEdit(EditAction::Revert(None)));
    }
    ui.same_line();
    let save = match sink {
        Sink::Ours => "Save screen files",
        Sink::Retail => "Save map",
    };
    if ui.button(format!("{} {save}", icon::SAVE)) {
        actions.push(Action::UiEdit(EditAction::Save));
    }
    match sink {
        Sink::Ours => {
            let ours = &app.ui_screens.edit.ours;
            match ours.checkout_path() {
                Some(c) => ui.text_disabled(format!("saves into {}", c.join(crate::app::ui_screens::edit::ours::SCREENS_DIR).display())),
                None => ui.text_disabled("no checkout found: Save is refused"),
            }
            let changed = ours.changed_on_disk();
            if !changed.is_empty() {
                ui.text_colored(
                    theme::WARN,
                    format!(
                        "changed on disk since: {} (Save overwrites, Revert screen takes the disk's)",
                        changed.join(", ")
                    ),
                );
            }
        }
        Sink::Retail => ui.text_disabled(
            "Retail edits are tag edits of mainmenu.map: they save with the map.",
        ),
    }
    if let Some(e) = &app.ui_screens.edit.last_error {
        ui.text_colored(theme::ERROR, icon::ALERT);
        ui.same_line();
        ui.text_wrapped(e);
    }
}

/// The widget list: each widget of the screen shown; hover lights it in
/// the picture, click selects it.
pub fn widget_list(ui: &Ui, app: &mut App, st: &mut EditUi, actions: &mut Vec<Action>) {
    st.list_hot = None;
    let Some(view) = app.ui_screens.edit_view().cloned() else {
        ui.text_disabled("Nothing drawn.");
        return;
    };
    let selected = app.ui_screens.edit.selected.clone();
    for w in &view.widgets {
        let label = match &w.kind {
            Kind::Text { text, .. } => {
                let short: String = text.chars().take(24).collect();
                format!("{}  {short}", w.id)
            }
            Kind::Art { .. } => w.id.clone(),
        };
        if ui
            .selectable_config(format!("{label}##ui-w-{}", w.id))
            .selected(selected.as_deref() == Some(w.id.as_str()))
            .build()
        {
            actions.push(Action::UiEdit(EditAction::Select(Some(w.id.clone()))));
        }
        if ui.is_item_hovered() {
            st.list_hot = Some(w.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_moves_whole_and_a_handle_its_edges() {
        // A rect as [top, left, bottom, right]: built from its edges so
        // the theme test doesn't take it for a colour.
        let (top, left, bottom, side): (i32, i32, i32, i32) = (100, -50, 80, 50);
        let r = [top, left, bottom, side].map(|v| v as i16);
        assert_eq!(moved(r, Grip::BODY, 3, -2), [top - 2, left + 3, bottom - 2, side + 3]);
        assert_eq!(moved(r, HANDLES[3], 10, 5), [top, left, bottom, side + 10]);
        // An edge stops at its opposite.
        assert_eq!(moved(r, HANDLES[1], 0, -40), [bottom, left, bottom, side]);
        assert_eq!(moved(r, HANDLES[6], -5, -5), [top, left - 5, bottom - 5, side]);
    }

    #[test]
    fn snapping_lands_on_near_edges_and_centres_only() {
        assert_eq!(snap_axis(&[10, 50], &[0, 13, 200]), 3);
        assert_eq!(snap_axis(&[10], &[20]), 0);
        assert_eq!(snap_axis(&[10, 52], &[13, 51]), -1);
        // Pixels as [left, top, width, height], built from parts so the
        // theme test doesn't take them for a colour.
        let (x, y, w, h) = (10, 20, 100, 40);
        let px = [x, y, w, h];
        assert_eq!(handle_at(HANDLES[3], px), [110.0, 40.0]);
        assert_eq!(handle_at(HANDLES[0], px), [10.0, 20.0]);
    }
}
