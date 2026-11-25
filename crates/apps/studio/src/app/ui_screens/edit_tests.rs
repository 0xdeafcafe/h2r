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
        std::fs::write(
            dir.join(ours::UI_MANIFEST),
            include_str!("../../../../../../content/ui/package.toml"),
        )
        .unwrap();
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
    app_on(t, SCREEN)
}

/// An app on `screen`'s first state, its checkout `t`.
fn app_on(t: &Temp, screen: &str) -> App {
    let mut app = App::new();
    app.ui_screens.edit.ours.set_checkout(Some(t.0.clone()));
    let state = UiScreens::tree()
        .into_iter()
        .find(|(n, _)| *n == screen)
        .map(|(_, s)| s[0].clone())
        .unwrap();
    app.dispatch(Action::SetTool(ToolKind::UiScreens));
    app.dispatch(Action::UiPick(Pick::Native {
        screen: screen.into(),
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

/// A settings dialog's header lines edit its `settings:` block (the
/// about line the caller's help when the dialog takes it from there), its
/// help block edits the shown row's help, and none of them moves: the
/// `settings` component places them.
#[test]
fn a_settings_dialogs_header_and_help_edit_its_doc() {
    let t = Temp::new("settings");
    let mut app = app_on(&t, "kit.dialog");
    let set = |app: &mut App, id: &str, text: &str| {
        edit(
            app,
            EditAction::SetText {
                id: id.into(),
                text: text.into(),
            },
        )
    };
    set(&mut app, "header.name", "Example").unwrap();
    set(&mut app, "header.about", "What the example is for.").unwrap();
    assert_eq!(widget(&mut app, "header.name").text(), Some("Example"));
    let base = app.ui_screens.edit.ours.doc("kit.dialog").unwrap().clone();
    let shown = widget(&mut app, "description").text().unwrap().replace('\n', " ");
    let (row, _) = base
        .rows
        .iter()
        .find(|(_, r)| r.help.as_deref() == Some(shown.as_str()))
        .expect("the help block shows a row's help");
    let row = row.clone();
    set(&mut app, "description", "How many play.").unwrap();
    let d = app.ui_screens.edit.ours.doc("kit.dialog").unwrap().clone();
    let st = d.settings.clone().unwrap();
    assert_eq!(st.name.as_deref(), Some("Example"));
    assert_eq!(st.about.as_deref(), Some("What the example is for."));
    assert_eq!(d.rows[&row].help.as_deref(), Some("How many play."));
    for (id, r) in &d.rows {
        if *id != row {
            assert_eq!(r.help, base.rows[id].help, "{id}: only the shown row's help");
        }
    }
    for id in ["header.name", "header.about", "description"] {
        let e = edit(
            &mut app,
            EditAction::SetRect {
                id: id.into(),
                rect: [170, -224, 150, 0],
                merge: false,
            },
        )
        .unwrap_err();
        assert!(e.starts_with("refused") && e.contains("components.settings"), "{id}: {e}");
    }
    let e = set(&mut app, "header.name", " ").unwrap_err();
    assert!(e.starts_with("refused"), "{e}");
    edit(&mut app, EditAction::Revert(Some("header.name".into()))).unwrap();
    edit(&mut app, EditAction::Revert(Some("description".into()))).unwrap();
    let d = app.ui_screens.edit.ours.doc("kit.dialog").unwrap().clone();
    assert_eq!(d.settings.unwrap().name.as_deref(), Some("Name"));
    assert_eq!(d.rows[&row].help, base.rows[&row].help);

    // Clankers takes its about line from the Game Setup row's help.
    let mut app = app_on(&t, "clankers");
    set(&mut app, "header.about", "Bots, in this game.").unwrap();
    let st = app.ui_screens.edit.ours.doc("clankers").unwrap().settings.clone().unwrap();
    assert_eq!(st.about, None);
    assert_eq!(st.caller.unwrap().help, "Bots, in this game.");
}

#[test]
fn save_changes_only_a_screen_source_in_the_ui_package() {
    let t = Temp::new("save");
    let path = t.0.join("content/ui/screens/kit.example.yaml");
    let before = include_str!("../../../../../../content/ui/screens/kit.example.yaml");
    std::fs::write(&path, before).unwrap();
    let mut app = app(&t);
    edit(
        &mut app,
        EditAction::SetText {
            id: "title".into(),
            text: "Saved title".into(),
        },
    )
    .unwrap();
    let old_manifest = std::fs::read(t.0.join(ours::UI_MANIFEST)).unwrap();
    edit(&mut app, EditAction::Save).unwrap();
    assert_eq!(std::fs::read(t.0.join(ours::UI_MANIFEST)).unwrap(), old_manifest);
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    assert!(!t.0.join("assets").exists());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_ne!(text, before);
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
    let old_manifest = std::fs::read(t.0.join(ours::UI_MANIFEST)).unwrap();
    edit(&mut app, EditAction::Save).unwrap();
    let path = t.0.join("content/ui/screens/kit.example.yaml");
    assert_eq!(std::fs::read(t.0.join(ours::UI_MANIFEST)).unwrap(), old_manifest);
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    assert!(!t.0.join("assets").exists());
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
    app.dispatch(Action::UiPick(Pick::retail(r"ui\screens\x")));
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
