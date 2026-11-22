//! The UI Screens tool's edits without a disc: our screens' sink over the
//! baked docs, a temporary checkout for the save, and the retail sink's
//! refusals.

use std::path::PathBuf;

use super::ours::{self, slots};
use super::*;
use crate::app::{Action, ToolKind};

const SCREEN: &str = "kit.example";

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-uiedit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(ours::SCREENS_DIR)).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app on our example screen's first state, its checkout `t`.
fn app(t: &Temp) -> App {
    let mut app = App::new();
    app.ui_screens.edit.ours.set_checkout(Some(t.0.clone()));
    let state = UiScreens::tree()
        .into_iter()
        .find(|(n, _)| *n == SCREEN)
        .map(|(_, s)| s[0].clone())
        .unwrap();
    app.dispatch(Action::SetTool(ToolKind::UiScreens));
    app.dispatch(Action::UiPick(Pick::Native {
        screen: SCREEN.into(),
        state,
    }));
    assert!(app.ui_screens.shot().is_some(), "{:?}", app.ui_screens.last);
    app
}

fn widget(app: &mut App, id: &str) -> Widget {
    app.ui_screens.edit_widget(id).unwrap()
}

fn edit(app: &mut App, a: EditAction) -> Result<(), String> {
    app.dispatch(Action::UiEdit(a));
    match app.ui_screens.edit.last_error.clone() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[test]
fn a_text_edit_shows_undoes_and_reverts() {
    let t = Temp::new("text");
    let mut app = app(&t);
    let before = widget(&mut app, "title").text().unwrap().to_owned();
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "Edited title".into(),
        },
    )
    .unwrap();
    assert_eq!(widget(&mut app, "title").text(), Some("Edited title"));
    assert_eq!(app.ui_screens.edit.selected.as_deref(), Some("title"));
    assert!(app.tool_dirty(ToolKind::UiScreens));
    assert!(app.can_undo());

    app.dispatch(Action::Undo);
    assert_eq!(widget(&mut app, "title").text(), Some(before.as_str()));
    assert!(!app.tool_dirty(ToolKind::UiScreens));
    app.dispatch(Action::Redo);
    assert_eq!(widget(&mut app, "title").text(), Some("Edited title"));

    edit(&mut app, EditAction::Revert(Some("title".into()))).unwrap();
    assert_eq!(widget(&mut app, "title").text(), Some(before.as_str()));
    // The revert is a step of its own.
    app.dispatch(Action::Undo);
    assert_eq!(widget(&mut app, "title").text(), Some("Edited title"));
    edit(&mut app, EditAction::Revert(None)).unwrap();
    assert!(!app.tool_dirty(ToolKind::UiScreens));
}

#[test]
fn a_drag_is_one_step_and_moves_the_widget() {
    let t = Temp::new("move");
    let mut app = app(&t);
    let [top, left, bottom, right] = widget(&mut app, "description").rect;
    for _ in 0..3 {
        edit(
            &mut app,
            EditAction::Move {
                id: "description".into(),
                dx: 2,
                dy: -1,
                merge: true,
            },
        )
        .unwrap();
    }
    edit(&mut app, EditAction::Seal).unwrap();
    // The title sits where the header layout puts it: it can't move.
    assert!(
        edit(
            &mut app,
            EditAction::Move {
                id: "title".into(),
                dx: 1,
                dy: 0,
                merge: false,
            },
        )
        .is_err()
    );
    assert_eq!(
        widget(&mut app, "description").rect,
        [top - 3, left + 6, bottom - 3, right + 6]
    );
    app.dispatch(Action::Undo);
    assert_eq!(widget(&mut app, "description").rect, [top, left, bottom, right]);
    assert!(!app.can_undo(), "the drag was one step");
}

#[test]
fn checks_run_on_the_edited_view() {
    let t = Temp::new("checks");
    let mut app = app(&t);
    let named = |app: &mut App| {
        app.ui_screens
            .shot()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.ids.iter().any(|i| i == "description"))
    };
    assert!(!named(&mut app));
    // Past the title-safe area's top (204) and left (-272).
    edit(
        &mut app,
        EditAction::SetRect {
            id: "description".into(),
            rect: [236, -316, 212, 24],
            merge: false,
        },
    )
    .unwrap();
    assert!(named(&mut app), "{:?}", app.ui_screens.shot().unwrap().findings);
}

#[test]
fn rust_owned_widgets_are_refused_with_their_owner() {
    let t = Temp::new("refused");
    let mut app = app(&t);
    let value = app
        .ui_screens
        .edit_view()
        .unwrap()
        .widgets
        .iter()
        .find(|w| w.id.ends_with(".value"))
        .map(|w| w.id.clone())
        .expect("the example has a value row");
    let e = edit(
        &mut app,
        EditAction::SetText {
            id: value.clone(),
            text: "x".into(),
        },
    )
    .unwrap_err();
    assert!(e.starts_with("refused") && e.contains("setting"), "{e}");
    let label = value.replace(".value", ".label");
    let e = edit(
        &mut app,
        EditAction::Move {
            id: label,
            dx: 1,
            dy: 0,
            merge: false,
        },
    )
    .unwrap_err();
    assert!(e.contains("layout"), "{e}");
    let e = edit(&mut app, EditAction::Select(Some("nope".into()))).unwrap_err();
    assert!(e.contains("no widget"), "{e}");
    let e = edit(
        &mut app,
        EditAction::SetRect {
            id: "title".into(),
            rect: [0, 10, 20, 0],
            merge: false,
        },
    )
    .unwrap_err();
    assert!(e.contains("top >= bottom"), "{e}");
    assert!(!app.tool_dirty(ToolKind::UiScreens));
}

#[test]
fn save_writes_the_yaml_into_the_checkout_only() {
    let t = Temp::new("save");
    let mut app = app(&t);
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "Saved title".into(),
        },
    )
    .unwrap();
    edit(&mut app, EditAction::Save).unwrap();
    let path = ours::file_of(&t.0, SCREEN);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Saved title"), "{text}");
    assert!(!app.tool_dirty(ToolKind::UiScreens));
    // Read back: the file is what the screen shows from now on.
    app.ui_screens.edit.ours.reload().unwrap();
    assert_eq!(widget(&mut app, "title").text(), Some("Saved title"));

    // No checkout: refused, the edit kept.
    app.ui_screens.edit.ours.set_checkout(None);
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "Kept".into(),
        },
    )
    .unwrap();
    let e = edit(&mut app, EditAction::Save).unwrap_err();
    assert!(e.starts_with("refused") && e.contains("checkout"), "{e}");
    assert!(app.tool_dirty(ToolKind::UiScreens));
}

#[test]
fn a_file_changed_on_disk_shows_unless_edited() {
    let t = Temp::new("reload");
    let mut app = app(&t);
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "On disk".into(),
        },
    )
    .unwrap();
    edit(&mut app, EditAction::Save).unwrap();
    let path = ours::file_of(&t.0, SCREEN);
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("On disk", "Changed outside")).unwrap();
    edit(&mut app, EditAction::Reload).unwrap();
    assert_eq!(widget(&mut app, "title").text(), Some("Changed outside"));
    // With an unsaved edit the edit stays and the change is named.
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "Mine".into(),
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("Changed outside", "Again")).unwrap();
    edit(&mut app, EditAction::Reload).unwrap();
    assert_eq!(widget(&mut app, "title").text(), Some("Mine"));
    assert_eq!(app.ui_screens.edit.ours.changed_on_disk(), vec![SCREEN.to_owned()]);
}

#[test]
fn a_filmstrip_spans_the_transition() {
    let t = Temp::new("film");
    let mut app = app(&t);
    let (strip, frames) = app.ui_screens.filmstrip(Transition::Open, 4).unwrap();
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0].tick, 0);
    assert_eq!(frames[3].tick, Transition::Open.ticks());
    assert_eq!(strip.width, 4 * (320 + 8) + 8);
    assert_eq!(frames[1].x, frames[0].x + frames[0].w + 8);
    assert!(app.ui_screens.filmstrip(Transition::Close, 0).is_err());
    // The onion skin's poses: the first fully hidden, the rest at rest.
    let poses = app
        .ui_screens
        .poses(&[Phase::In(0), Phase::Rest]);
    assert_eq!(poses.len(), 2);
    assert_eq!(poses[0].widget("title").unwrap().alpha, 0);
    assert_eq!(poses[1].widget("title").unwrap().alpha, 255);
}

#[test]
fn retail_edits_need_the_disc_and_the_menu_map() {
    let mut app = App::new();
    app.dispatch(Action::SetTool(ToolKind::UiScreens));
    app.dispatch(Action::UiPick(Pick::Retail(r"ui\screens\x".into())));
    let e = edit(
        &mut app,
        EditAction::Move {
            id: "text.0".into(),
            dx: 1,
            dy: 0,
            merge: false,
        },
    )
    .unwrap_err();
    assert!(e.contains("nothing is shown"), "{e}");
    assert!(!app.ui_can_undo_redo(false));
}

#[test]
fn template_slots_are_found() {
    let s = slots("Welcome {gamertag}, {n} of {total} {{x");
    assert_eq!(
        s.into_iter().collect::<Vec<_>>(),
        ["gamertag", "n", "total"]
    );
    assert!(slots("no slots {} here").is_empty());
}
