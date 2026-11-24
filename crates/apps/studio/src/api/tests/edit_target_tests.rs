//! Edits go to the edit target (docs/editor/studio-redesign.md, "Editing a
//! disc asset"): the window's field row and `tag.set` write the same edit
//! line, an undo takes it out after a save, and with no target the API
//! answers `NO_TARGET` while the window asks for one. Over the Tags tool's
//! invented weapon, no game data.

use std::path::{Path, PathBuf};

use super::*;
use crate::app::edit_target::NO_TARGET;
use crate::app::tag_tests::{app as tags_app, weap_id};
use crate::app::{EditChoice, package_doc};

fn weap_ref(app: &App) -> String {
    let t = app.tags.tag(weap_id()).unwrap();
    format!("weap:{}", t.name)
}

/// An app with the weapon open, edits going to the target, and package
/// `mine` as the target when `with_target`.
fn open(t: &Temp, with_target: bool) -> App {
    let (mut app, _, _) = tags_app(&t.0);
    app.edits_to_target();
    if with_target {
        let data = app.data_root();
        package_doc::new(&data, "mine", "Mine").unwrap();
        package_doc::edit_target(&data, Some("mine"), false).unwrap();
    }
    app
}

fn edits_path(app: &App) -> PathBuf {
    app.data_root().join("packages/mine/edits/example.yaml")
}

/// The window's field row writing a float: bytes at the field's address.
fn window_set_speed(app: &mut App, v: f32) {
    let at = app.tags.tag(weap_id()).unwrap().address;
    app.dispatch(crate::fields::write_action(
        weap_id(),
        at,
        v.to_le_bytes().to_vec(),
        "speed",
    ));
    app.dispatch(Action::Tag(crate::app::TagAction::Seal));
}

fn api_set_speed(app: &mut App, v: f64) -> Reply {
    let tag = weap_ref(app);
    Api::new().call(
        app,
        m::TAG_SET,
        json!({"tag": tag, "path": "speed", "value": v}),
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn a_field_set_in_the_window_and_by_tag_set_write_the_same_line() {
    let (ta, tb) = (Temp::new("edit-window"), Temp::new("edit-api"));
    let mut window = open(&ta, true);
    let mut apiapp = open(&tb, true);
    window_set_speed(&mut window, 4.0);
    api_set_speed(&mut apiapp, 4.0).unwrap();

    let (a, b) = (read(&edits_path(&window)), read(&edits_path(&apiapp)));
    assert!(!a.is_empty(), "the window's edit wrote a line");
    assert_eq!(a, b);
    let lines = ::workspace::parse_edits(&a).unwrap();
    assert_eq!(lines.len(), 1);
    let (edit, _) = &lines[0];
    assert_eq!(edit.tag, weap_ref(&window));
    assert_eq!(edit.set.len(), 1);
    assert_eq!(edit.set[0].path, "speed");
    assert_eq!(edit.set[0].value.as_f64(), Some(4.0));

    // The package holds it as an `edits` item for the map, locked.
    let pkg = package_doc::read(&window.data_root(), "mine").unwrap();
    assert_eq!(pkg["items"][0]["kind"], "edits");
    assert_eq!(pkg["items"][0]["source"], "edits/example.yaml");
}

#[test]
fn an_undo_after_a_save_takes_the_line_out() {
    let t = Temp::new("edit-undo");
    let mut app = open(&t, true);
    api_set_speed(&mut app, 9.0).unwrap();
    app.dispatch(Action::Save);
    assert!(read(&edits_path(&app)).contains("speed"));

    app.dispatch(Action::Undo);
    assert!(!edits_path(&app).exists(), "the file goes with its last line");
    let pkg = package_doc::read(&app.data_root(), "mine").unwrap();
    assert_eq!(pkg["items"].as_array().unwrap().len(), 0);

    app.dispatch(Action::Redo);
    assert!(read_after(&app).contains("speed"));
}

fn read_after(app: &App) -> String {
    read(&edits_path(app))
}

#[test]
fn with_no_target_the_api_answers_no_target_and_the_window_asks() {
    let t = Temp::new("edit-no-target");
    let mut app = open(&t, false);
    let before = app.tags.read(weap_id(), app.tags.tag(weap_id()).unwrap().address, 4);

    let e = api_set_speed(&mut app, 4.0).unwrap_err();
    assert_eq!(e.code, code::NO_TARGET);
    assert_eq!(e.message, NO_TARGET);
    assert_eq!(
        app.tags.read(weap_id(), app.tags.tag(weap_id()).unwrap().address, 4),
        before,
        "nothing was written"
    );

    window_set_speed(&mut app, 4.0);
    assert!(app.edits.picker.is_some(), "the first edit asks for a target");
    assert_eq!(
        app.tags.read(weap_id(), app.tags.tag(weap_id()).unwrap().address, 4),
        before
    );

    // A new package is made, becomes the target and takes the held change.
    app.dispatch(Action::ChooseEditTarget(EditChoice::New("Mine".into())));
    assert!(app.edits.picker.is_none());
    assert_eq!(app.edit_target().as_deref(), Some("mine"));
    assert!(read(&edits_path(&app)).contains("speed"));
}

#[test]
fn cancelling_the_picker_drops_the_held_change() {
    let t = Temp::new("edit-cancel");
    let mut app = open(&t, false);
    window_set_speed(&mut app, 4.0);
    app.dispatch(Action::ChooseEditTarget(EditChoice::Cancel));
    assert!(app.edits.picker.is_none());
    assert!(app.edit_target().is_none());
    assert!(!app.tags.has_edits());
}

#[test]
fn lines_of_tags_this_session_has_not_changed_stay() {
    let t = Temp::new("edit-keep");
    let mut app = open(&t, true);
    let data = app.data_root();
    let other = "- tag: 'weap:invented\\other'\n  set:\n  - path: speed\n    value: 1.0\n";
    package_doc::put_edits(&data, "mine", "example", other).unwrap();
    api_set_speed(&mut app, 4.0).unwrap();
    let text = read(&edits_path(&app));
    let lines = ::workspace::parse_edits(&text).unwrap();
    let tags: Vec<&str> = lines.iter().map(|(e, _)| e.tag.as_str()).collect();
    assert_eq!(tags.len(), 2, "{text}");
    assert!(tags.contains(&"weap:invented\\other"));
    assert!(tags.contains(&weap_ref(&app).as_str()));
}
