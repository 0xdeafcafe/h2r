//! The object actions menu (docs/game/ui-screens.md, "The floating
//! menu"): Edit, Duplicate, Delete and Properties for the aimed or held
//! object, as a floating menu at the object.
//!
//! The menu is `content/ui/screens/forge.object-actions.yaml`: its
//! `floating:` block names the anchor (the object's point in the world),
//! the corner's offset from it and the actions. [`sheet`] gives each
//! action what it does for the object; nothing here names a row's words.
//!
//! The anchor is the object's point as the window shows it. The session
//! says where that is each tick ([`aim`]: the monitor camera's projection,
//! `crate::forge_glue::editing`). [`place`] puts the list there as the
//! screen opens (`crate::list_class::move_list`), through the one anchor
//! rule (`crate::ui_kit::floating::window_point`, `canvas_fit`), kept
//! inside the title-safe area. While the menu is up [`follow`] gives how
//! far the point has moved since, and the UI update moves the drawn list
//! by that (`crate::list_class::offset_list`). What it shows is published
//! for `ui.stack` ([`crate::ui_kit::floating::shown`]).

use std::sync::Mutex;

use forge::Endpoint;

use super::OBJECT_ACTIONS_SCREEN as SCREEN;
use super::sheet::{Act, Choice, Kind, Sheet, SheetRow};
use crate::list_class::{self, ListLayout};
use crate::ui_kit::data;
use crate::ui_kit::floating::{self, Menu};

/// Where the menu is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Held {
    /// The object's point in the window, normalised device coordinates
    /// (-1 to 1, Y up); `None` behind the camera or with no object.
    ndc: Option<[f32; 2]>,
    /// The list's anchor as the open screen's tag has it.
    opened: Option<[i16; 2]>,
}

static HELD: Mutex<Held> = Mutex::new(Held {
    ndc: None,
    opened: None,
});

fn held() -> std::sync::MutexGuard<'static, Held> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())
}

/// The menu its doc declares.
fn menu() -> Option<Menu> {
    Some(Menu::of(data::screen(SCREEN)?.floating.as_ref()?))
}

/// The object's point in the window now: the session's, each tick.
pub fn aim(ndc: Option<[f32; 2]>) {
    held().ndc = ndc.filter(|p| p.iter().all(|v| v.is_finite()));
}

/// The list's anchor for a menu of `rows` rows with the object at `ndc`
/// on `output`, whether it was moved to stay inside the safe area, and
/// the object's point in UI units. A point behind the camera is the
/// canvas's centre.
fn anchor_at(
    menu: &Menu,
    ndc: Option<[f32; 2]>,
    output: (u32, u32),
    rows: usize,
) -> ([i16; 2], bool, Option<[f32; 2]>) {
    let point = ndc.map(|p| floating::window_point(p, output));
    let (anchor, moved) = floating::place(menu.point(point, output), menu.offset, rows);
    (anchor, moved, point)
}

/// The list's layout for `rows` rows: the default skin with no frame
/// art, at the open menu's anchor, or where the object's point puts it
/// now.
pub fn layout(rows: usize) -> ListLayout {
    let h = *held();
    let anchor = h.opened.unwrap_or_else(|| match menu() {
        Some(menu) => anchor_at(&menu, h.ndc, crate::render_aspect::layout_output(), rows).0,
        None => floating::place([0.0, 0.0], [0, 0], rows).0,
    });
    ListLayout {
        skin: 0,
        anchor,
        bitmaps_left_of: i16::MIN,
        visible: rows.clamp(1, floating::MAX_ACTIONS) as i16,
    }
}

fn publish(menu: &Menu, point: Option<[f32; 2]>, output: (u32, u32), labels: Option<&Sheet>) {
    let mut shown = floating::Shown::of(SCREEN, menu, point, output, 0);
    if let Some(sheet) = labels {
        shown.focus = sheet.focus;
        shown.actions = sheet.rows.iter().map(|r| r.label.clone()).collect();
    }
    floating::publish(Some(shown));
}

/// Places the list of the menu about to open, of `rows` rows, at the
/// object's point.
pub(super) fn place(rows: usize) {
    let Some(menu) = menu() else {
        return;
    };
    let output = crate::render_aspect::layout_output();
    let ndc = held().ndc;
    let (anchor, _, point) = anchor_at(&menu, ndc, output, rows);
    held().opened = Some(anchor);
    list_class::move_list(SCREEN, Some(anchor));
    publish(
        &menu,
        point,
        output,
        Some(&super::screen(Kind::ObjectActions).sheet()),
    );
}

/// How far the open menu's list is from where it opened, for a menu of
/// `rows` rows (`x, y`, Y up): the object's point moved, or the window
/// changed shape. `None` while no menu has been placed.
pub(super) fn follow(rows: usize) -> Option<[f32; 2]> {
    let menu = menu()?;
    let h = *held();
    let opened = h.opened?;
    let output = crate::render_aspect::layout_output();
    let (anchor, _, point) = anchor_at(&menu, h.ndc, output, rows);
    publish(
        &menu,
        point,
        output,
        Some(&super::screen(Kind::ObjectActions).sheet()),
    );
    Some([
        f32::from(anchor[0]) - f32::from(opened[0]),
        f32::from(anchor[1]) - f32::from(opened[1]),
    ])
}

/// The menu's screen was freed.
pub(super) fn closed() {
    held().opened = None;
    floating::publish(None);
}

/// The menu for the object `id` named `name`: a row for each of the
/// doc's actions, doing for this object what the action names. Edit
/// opens the Coordinates editor (its sheet is shown by the session
/// first), Properties opens `properties` above the menu, Duplicate and
/// Delete choose and close. An action that opens Properties is left out
/// when the object has none.
pub fn sheet(name: &str, id: Endpoint, properties: Option<Sheet>) -> Result<Sheet, String> {
    let menu = menu().ok_or_else(|| format!("{SCREEN} has no floating menu in its doc."))?;
    let mut rows = Vec::new();
    for a in &menu.actions {
        let opens = a.open.as_deref().and_then(Kind::of_screen);
        let act = match (a.act.as_deref(), opens) {
            (Some("duplicate"), _) => Act::Choose(Choice::Duplicate(id)),
            (Some("delete"), _) => Act::Choose(Choice::Delete(id)),
            (None, Some(Kind::Coordinates)) => Act::Open(Kind::Coordinates),
            (None, Some(Kind::Properties)) => match &properties {
                Some(sheet) => Act::Dialog(Box::new(sheet.clone())),
                None => continue,
            },
            (act, _) => {
                return Err(format!(
                    "{SCREEN}: {:?} names nothing an object does ({:?})",
                    a.text,
                    act.or(a.open.as_deref()).unwrap_or_default()
                ));
            }
        };
        rows.push(SheetRow::new(&a.text, act));
    }
    Ok(Sheet {
        kind: Kind::ObjectActions,
        title: name.to_owned(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// A crate's menu, for the preview and the tests.
pub fn sample() -> Sheet {
    let properties = Sheet {
        kind: Kind::Properties,
        title: "CRATE".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    };
    sheet("CRATE", Endpoint::Added(forge::OpId(0)), Some(properties)).expect("the doc's actions")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The doc's four actions, each doing its own thing for the object.
    #[test]
    fn each_action_of_the_doc_acts_on_the_object() {
        let id = Endpoint::Added(forge::OpId(7));
        let properties = Sheet {
            kind: Kind::Properties,
            title: "Crate".into(),
            body: String::new(),
            rows: Vec::new(),
            focus: 0,
        };
        let s = sheet("Crate", id, Some(properties.clone())).unwrap();
        assert_eq!(
            (s.kind, s.title.as_str(), s.focus),
            (Kind::ObjectActions, "Crate", 0)
        );
        let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Edit", "Duplicate", "Delete", "Properties"]);
        assert_eq!(s.rows[0].act, Act::Open(Kind::Coordinates));
        assert_eq!(s.rows[1].act, Act::Choose(Choice::Duplicate(id)));
        assert_eq!(s.rows[2].act, Act::Choose(Choice::Delete(id)));
        assert_eq!(s.rows[3].act, Act::Dialog(Box::new(properties)));
        // An object with no properties has no Properties row.
        let bare = sheet("Crate", id, None).unwrap();
        assert_eq!(bare.rows.len(), 3);
        assert!(s.rows.len() <= floating::MAX_ACTIONS);
    }

    /// The list goes where the object is drawn, by the one anchor rule:
    /// the same canvas point at 4:3, and the point the fit draws there on
    /// a wide window; a point off the safe area is brought inside.
    #[test]
    fn the_list_is_anchored_at_the_objects_point_inside_the_safe_area() {
        let menu = menu().expect("the doc");
        let rows = menu.actions.len();
        // The window's centre is the canvas's centre on every window.
        for output in [(640, 480), (1920, 1080), (3440, 1440)] {
            let (anchor, moved, point) = anchor_at(&menu, Some([0.0, 0.0]), output, rows);
            assert_eq!(
                point.map(|p| p.map(f32::round)),
                Some([0.0, 0.0]),
                "{output:?}"
            );
            assert_eq!(
                anchor,
                floating::place([0.0, 0.0], menu.offset, rows).0,
                "{output:?}"
            );
            assert!(!moved, "{output:?}");
        }
        // Half way to the left edge: 160 units left at 4:3, further in
        // canvas units on a wide window, whose canvas is drawn in its
        // middle.
        let (narrow, ..) = anchor_at(&menu, Some([-0.5, 0.0]), (640, 480), rows);
        let (wide, ..) = anchor_at(&menu, Some([-0.5, 0.0]), (1920, 1080), rows);
        assert_eq!(narrow[0], -160 + menu.offset[0]);
        assert!(wide[0] < narrow[0], "{wide:?} against {narrow:?}");
        // The window's corner is off the safe area: the menu is moved in.
        let (corner, moved, _) = anchor_at(&menu, Some([1.0, -1.0]), (1920, 1080), rows);
        assert!(moved);
        let [top, left, bottom, right] = floating::ROOM;
        assert!(corner[0] >= left && corner[0] <= right, "{corner:?}");
        assert!(corner[1] <= top && corner[1] >= bottom, "{corner:?}");
        // Behind the camera: the canvas's centre.
        let (behind, ..) = anchor_at(&menu, None, (1920, 1080), rows);
        assert_eq!(behind, floating::place([0.0, 0.0], menu.offset, rows).0);
    }
}
