//! Compiling into the map over the scenario crate's invented map with
//! invented script tables: dry runs, in-place edits, growth, undo, the
//! working copy, and the saved map read back. No game data.
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::api::Api;
use crate::app::map_cards_tests::Temp;
use crate::app::scripts::tests as scripts;
use crate::app::{Action, App, Doc, ToolKind};

fn open(t: &Temp, executable: bool) -> App {
    let map = scripts::scripted_source(&t.0, executable);
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    app
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    api.call(app, method, params).unwrap()
}

fn source_of(api: &mut Api, app: &mut App, name: &str) -> String {
    call(api, app, m::SCRIPT_SOURCE, json!({"name": name}))["source"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `helper` calling `invented_watch` on the volume `main` already names:
/// the same three nodes, and no name or string the map lacks.
const WATCHING: &str = "(script static void helper\n  (invented_watch zone))";

#[test]
fn a_check_says_what_a_compile_would_do_and_changes_nothing() {
    let t = Temp::new("compile-check");
    let mut app = open(&t, true);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::SCRIPT_COMPILE, json!({"source": WATCHING}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["applied"], false);
    assert_eq!(v["definitions"], json!([{"name": "helper", "item": "script"}]));
    assert_eq!((v["nodes"].as_u64(), v["freed"].as_u64(), v["reused"].as_u64()), (Some(3), Some(3), Some(3)));
    assert_eq!((v["grown"].as_u64(), v["new_strings"].as_u64()), (Some(0), Some(0)));
    assert_eq!(v["grows"], false);
    assert!(!app.session.is_dirty());
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_select door"));
    assert_eq!(v["depth"], json!({"undo": 0, "redo": 0}));
}

#[test]
fn problems_come_back_with_their_place_and_nothing_is_written() {
    let t = Temp::new("compile-problems");
    let mut app = open(&t, true);
    let mut api = Api::new();
    let v = call(
        &mut api,
        &mut app,
        m::SCRIPT_COMPILE,
        json!({"source": "(script static void helper\n  (no_such_thing))", "apply": true}),
    );
    assert_eq!((v["ok"].clone(), v["applied"].clone()), (json!(false), json!(false)));
    let e = &v["errors"][0];
    assert_eq!((e["line"].as_u64(), e["column"].as_u64()), (Some(2), Some(4)), "{v}");
    assert!(e["text"].as_str().unwrap().contains("no_such_thing"));
    assert!(e["code"].as_str().is_some());
    assert!(!app.session.is_dirty());
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_select"));
}

#[test]
fn a_compile_that_fits_is_in_place_one_undo_step_and_saves() {
    let t = Temp::new("compile-apply");
    let mut app = open(&t, true);
    let mut api = Api::new();
    let free = call(&mut api, &mut app, m::SCRIPT_LIST, json!({}))["free_nodes"].clone();
    let v = call(
        &mut api,
        &mut app,
        m::SCRIPT_COMPILE,
        json!({"source": WATCHING, "apply": true, "name": "helper"}),
    );
    assert_eq!((v["ok"].clone(), v["applied"].clone(), v["grows"].clone()), (json!(true), json!(true), json!(false)), "{v}");
    assert_eq!(v["depth"], json!({"undo": 1, "redo": 0}));
    assert!(app.session.is_dirty());
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_watch zone"));
    // The other definitions read as they did, and the free slots are the
    // ones the map had.
    assert!(source_of(&mut api, &mut app, "main").contains("(helper)"));
    assert_eq!(call(&mut api, &mut app, m::SCRIPT_LIST, json!({}))["free_nodes"], free);
    // No table is copied: every patch lies in the node table, but for the
    // replaced script's root (it takes a new salt), in the script table.
    let c = app.session.scenario().unwrap().changes();
    assert!(c.tag_grown.is_empty() && !c.patches.is_empty());
    let doc = app.scripts.edit.as_ref().unwrap();
    let word = |o: usize| u32::from_le_bytes(doc.header[o..o + 4].try_into().unwrap());
    let (count, nodes) = (word(0x238), word(0x238 + 4));
    let (scripts, scripts_at) = (word(0x1b8), word(0x1b8 + 4));
    for (a, b) in &c.patches {
        let end = *a + b.len() as u32;
        let in_nodes = *a >= nodes && end <= nodes + count * 20;
        let in_scripts = *a >= scripts_at && end <= scripts_at + scripts * 40;
        assert!(in_nodes || in_scripts, "{a:#x}");
    }

    // Saved, the map reads the edit.
    let out = t.0.join("saved");
    std::fs::create_dir_all(&out).unwrap();
    app.session.save_as(out.join("example.map")).unwrap();
    std::fs::copy(t.0.join("default.xbe"), out.join("default.xbe")).unwrap();
    let mut again = App::new();
    again.set_data_root(t.0.join("data2"));
    again.dispatch(Action::OpenSource(out.join("example.map")));
    assert!(source_of(&mut api, &mut again, "helper").contains("invented_watch zone"));

    // Undo puts the table and the writes back; redo does it again. The
    // last save holds the edit: the undone map differs from it, and the
    // redone one is the saved state again.
    let u = call(&mut api, &mut app, m::SCRIPT_UNDO, json!({}));
    assert_eq!(u["changed"], true);
    assert_eq!(u["depth"], json!({"undo": 0, "redo": 1}));
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_select door"));
    assert!(app.session.is_dirty(), "undone from the saved map");
    assert_eq!(call(&mut api, &mut app, m::SCRIPT_UNDO, json!({}))["changed"], false);
    assert_eq!(call(&mut api, &mut app, m::SCRIPT_UNDO, json!({"redo": true}))["changed"], true);
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_watch zone"));
    assert!(!app.session.is_dirty(), "the saved state again");
}

#[test]
fn a_new_script_grows_a_table_and_needs_growth_on() {
    let t = Temp::new("compile-grow");
    let mut app = open(&t, true);
    let mut api = Api::new();
    let new = "(script static void extra\n  (invented_select door))";
    // The check reports the growth; applying is refused while it is off.
    let v = call(&mut api, &mut app, m::SCRIPT_COMPILE, json!({"source": new}));
    assert_eq!(v["grows"], true, "{v}");
    assert_eq!(v["definitions"], json!([{"name": "extra", "item": "script"}]));
    let e = api
        .call(&mut app, m::SCRIPT_COMPILE, json!({"source": new, "apply": true}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("full"), "{}", e.message);
    assert!(!app.session.is_dirty());
    assert!(call(&mut api, &mut app, m::SCRIPT_LIST, json!({}))["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["name"] != "extra"));

    app.session.set_allow_growth(true);
    let v = call(&mut api, &mut app, m::SCRIPT_COMPILE, json!({"source": new, "apply": true}));
    assert_eq!(v["ok"], true, "{v}");
    assert!(app.session.is_dirty());
    assert!(!app.session.scenario().unwrap().changes().tag_grown.is_empty());
    let out = t.0.join("saved");
    std::fs::create_dir_all(&out).unwrap();
    app.session.save_as(out.join("example.map")).unwrap();
    std::fs::copy(t.0.join("default.xbe"), out.join("default.xbe")).unwrap();
    let mut again = App::new();
    again.set_data_root(t.0.join("data2"));
    again.dispatch(Action::OpenSource(out.join("example.map")));
    let list = call(&mut api, &mut again, m::SCRIPT_LIST, json!({}));
    let names: Vec<_> = list["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["helper", "main", "extra"]);
    assert!(source_of(&mut api, &mut again, "extra").contains("invented_select door"));
}

#[test]
fn the_request_is_checked_before_the_compiler_is() {
    let t = Temp::new("compile-params");
    let mut app = open(&t, true);
    let mut api = Api::new();
    for params in [
        json!({}),
        json!({"source": WATCHING, "working": true}),
        json!({"source": WATCHING, "name": "main"}),
    ] {
        let e = api.call(&mut app, m::SCRIPT_COMPILE, params.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{params}: {}", e.message);
    }
    // No working copy is open.
    let e = api
        .call(&mut app, m::SCRIPT_COMPILE, json!({"working": true}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("script.external"), "{}", e.message);
}

#[test]
fn without_the_executable_or_a_map_it_is_refused() {
    let t = Temp::new("compile-noexe");
    let mut api = Api::new();
    let mut app = open(&t, false);
    let e = api
        .call(&mut app, m::SCRIPT_COMPILE, json!({"source": WATCHING}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("executable"), "{}", e.message);
    let mut none = App::new();
    let e = api
        .call(&mut none, m::SCRIPT_COMPILE, json!({"source": WATCHING}))
        .unwrap_err();
    assert_eq!(e.code, code::NO_DOCUMENT);
}

/// A save in an outside editor: `text`, with a modification time ahead.
fn save_as_editor(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
}

#[test]
fn the_working_copys_changes_compile_and_the_ones_it_left_alone_are_not_touched() {
    let t = Temp::new("compile-working");
    let mut app = open(&t, true);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::SCRIPT_EXTERNAL, json!({"launch": false}));
    let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
    // Unedited, it holds nothing to compile.
    let e = api
        .call(&mut app, m::SCRIPT_COMPILE, json!({"working": true}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("no edits"), "{}", e.message);

    let text = std::fs::read_to_string(&path).unwrap();
    save_as_editor(&path, &text.replace("(invented_select door)", "(invented_watch zone)"));
    let v = call(&mut api, &mut app, m::SCRIPT_COMPILE, json!({"working": true, "apply": true}));
    assert_eq!(v["definitions"], json!([{"name": "helper", "item": "script"}]), "{v}");
    assert!(source_of(&mut api, &mut app, "helper").contains("invented_watch zone"));
    assert!(app.session.is_dirty());
    // The tab's differences follow: the file no longer differs.
    let w = call(&mut api, &mut app, m::SCRIPT_WORKING, json!({}));
    assert_eq!(w["changed"], json!([]), "{w}");
}

#[test]
fn the_tools_compile_action_reports_in_the_tab_and_undoes() {
    let t = Temp::new("compile-action");
    let mut app = open(&t, true);
    app.dispatch(Action::Script(crate::app::ScriptAction::Compile {
        source: "(script static void helper (no_such_thing))".to_owned(),
        apply: true,
    }));
    assert!(matches!(
        &app.scripts.compile,
        Some(crate::app::script_edit::Outcome::Failed(crate::app::script_edit::Refusal::Problems(_)))
    ));
    app.dispatch(Action::Script(crate::app::ScriptAction::Compile {
        source: WATCHING.to_owned(),
        apply: true,
    }));
    assert!(matches!(
        &app.scripts.compile,
        Some(crate::app::script_edit::Outcome::Done(r)) if r.applied
    ));
    assert_eq!(app.scripts.compiles, 2);
    app.dispatch(Action::Script(crate::app::ScriptAction::CompileUndo { redo: false }));
    assert!(!app.session.is_dirty());

    // The compiles are the map document's steps while the Scripts surface
    // shows it (docs/editor/studio-flow.md, "Shared patterns"): the button's
    // `UndoDoc` and the shortcut's `Undo` both take the compile.
    app.dispatch(Action::Script(crate::app::ScriptAction::CompileUndo {
        redo: true,
    }));
    assert!(app.session.is_dirty());
    app.dispatch(Action::SetTool(ToolKind::Scripts));
    assert_eq!(app.doc_can(Doc::Map), (true, false));
    assert_eq!((app.can_undo(), app.can_redo()), (true, false));
    app.dispatch(Action::UndoDoc(Doc::Map));
    assert!(!app.session.is_dirty());
    assert_eq!(app.doc_can(Doc::Map), (false, true));
    // With nothing to undo, the button logs what the shortcut does.
    app.dispatch(Action::UndoDoc(Doc::Map));
    assert!(!app.session.is_dirty());
    assert_eq!(app.doc_can(Doc::Map), (false, true));
    assert!(
        app.status()
            .is_some_and(|s| s.text().contains("no compile to undo")),
        "{:?}",
        app.log()
            .iter()
            .map(|l| l.status.text())
            .collect::<Vec<_>>()
    );
    // Cmd+Shift+Z in the surface redoes the compile.
    app.dispatch(Action::Redo);
    assert!(app.session.is_dirty());
    assert_eq!(app.doc_can(Doc::Map), (true, false));
}
