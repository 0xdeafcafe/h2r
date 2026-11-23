//! The working copy VS Code and Zed edit (`app::external`), over the
//! invented scripted map and executable (`scripts::tests`). No game data.
use std::time::{Duration, Instant, SystemTime};

use hs::ide::{self, Context};

use super::*;
use crate::app::map_cards_tests::Temp;
use crate::app::scripts::tests::scripted_source;
use crate::app::{Action, App};

fn open(t: &Temp) -> App {
    let map = scripted_source(&t.0, true);
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    app
}

/// An outside editor's save: `text`, with a modification time `secs`
/// ahead, so the watch sees a new one.
fn save(path: &Path, text: &str, secs: u64) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(secs))
        .unwrap();
}

fn working(app: &App) -> &WorkingCopy {
    app.scripts.external.as_ref().expect("a working copy")
}

#[test]
fn the_working_copy_holds_every_definition_and_its_context() {
    let t = Temp::new("hs-working");
    let mut app = open(&t);
    let (path, line) = app.script_working_copy(Some("main")).unwrap();
    let l = app.scripts_loaded().unwrap();
    assert_eq!(path, working_path(&t.0.join("data"), &l.map));
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, working_text(l));
    for e in &l.entries {
        assert!(text.contains(&e.source.source), "{} in {text}", e.source.name);
    }
    let at = text.lines().nth(line as usize - 1).unwrap();
    assert!(at.starts_with("(script") && at.contains("main"), "line {line}: {at}");

    // The context names the builtins, the script kinds and the scenario's
    // names the scripts spell.
    let ctx = Context::parse(&std::fs::read_to_string(path.with_file_name(ide::CONTEXT_FILE)).unwrap()).unwrap();
    assert_eq!(ctx.map, l.map);
    assert!(ctx.builtins.iter().any(|b| b.name.as_deref() == Some("invented_select")));
    assert!(ctx.script_kinds.iter().any(|k| k == "static"));
    assert!(ctx.names_for("object_name").unwrap().iter().any(|n| n == "door"));
    assert!(ctx.names_for("trigger_volume").unwrap().iter().any(|n| n == "zone"));

    // As the map has it: no edits, and no errors against its own context.
    let w = working(&app);
    assert!(w.changes.is_empty(), "{:?}", w.changes);
    assert!(
        w.problems.iter().all(|p| p.severity != ide::severity::ERROR),
        "{:?}",
        w.problems
    );
    assert!(!w.stale);
    assert_eq!(std::fs::read_to_string(path.with_file_name(BASE_FILE)).unwrap(), text);
}

#[test]
fn a_save_in_the_outside_editor_is_read_back_and_compared_with_the_map() {
    let t = Temp::new("hs-save");
    let mut app = open(&t);
    let (path, _) = app.script_working_copy(None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!app.check_working_copy(Instant::now(), true), "nothing new");

    // Layout is no change; a new script is one.
    let mine = format!(
        "{}\n(script static void invented_added\n  (helper))\n",
        text.replace("(invented_select door)", "(invented_select   door)")
    );
    save(&path, &mine, 5);
    assert!(app.check_working_copy(Instant::now(), true));
    let w = working(&app);
    assert_eq!(w.text, mine);
    assert_eq!(
        w.changes,
        Changes {
            changed: vec![],
            added: vec!["invented_added".into()],
            removed: vec![],
        }
    );
    assert!(w.summary().contains("invented_added added"), "{}", w.summary());
    assert!(w.summary().contains("script compiler"), "{}", w.summary());

    // An edit to helper naming an object the scenario does not have, and
    // the global gone.
    let count = app
        .scripts_loaded()
        .unwrap()
        .entries
        .iter()
        .find(|e| e.source.name == "count")
        .unwrap()
        .source
        .source
        .clone();
    let theirs = text
        .replace("(invented_select door)", "(invented_select invented_nobody)")
        .replace(&count, "");
    save(&path, &theirs, 10);
    assert!(app.check_working_copy(Instant::now(), true));
    let w = working(&app);
    assert_eq!(w.changes.changed, ["helper"]);
    assert_eq!(w.changes.removed, ["count"]);
    assert!(w.changes.added.is_empty());
    assert!(
        w.problems.iter().any(|p| p.code == "name.unknown"),
        "{:?}",
        w.problems
    );

    // Opening it again keeps the edits.
    let (again, _) = app.script_working_copy(None).unwrap();
    assert_eq!(again, path);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
    assert_eq!(working(&app).changes.removed, ["count"]);
}

#[test]
fn studio_rewrites_an_unedited_working_copy_and_leaves_edits_alone() {
    let t = Temp::new("hs-refresh");
    let mut app = open(&t);
    let (path, _) = app.script_working_copy(None).unwrap();
    let fresh = std::fs::read_to_string(&path).unwrap();
    let base = path.with_file_name(BASE_FILE);

    // As if the map's scripts were others when the copy was written, and
    // the copy holds no edits: the map's scripts are read again and
    // written over it.
    let older = fresh.replace("(invented_select door)", "(invented_select zone)");
    std::fs::write(&path, &older).unwrap();
    {
        let w = app.scripts.external.as_mut().unwrap();
        w.written = older.clone();
        w.text = older.clone();
        w.watch.seen = modified(&path);
        w.for_rev = None;
    }
    assert!(app.check_working_copy(Instant::now(), true));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), fresh);
    assert_eq!(std::fs::read_to_string(&base).unwrap(), fresh);
    let w = working(&app);
    assert_eq!(w.text, fresh);
    assert!(!w.stale);

    // The same with edits in the copy: they stay, marked stale.
    let edited = format!("{older}\n(script static void invented_mine\n  (helper))\n");
    std::fs::write(&path, &edited).unwrap();
    {
        let w = app.scripts.external.as_mut().unwrap();
        w.written = older.clone();
        w.text = edited.clone();
        w.watch.seen = modified(&path);
        w.for_rev = None;
    }
    assert!(app.check_working_copy(Instant::now(), true));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    let w = working(&app);
    assert!(w.stale);
    assert_eq!(w.changes.added, ["invented_mine"]);
}

#[test]
fn the_working_copy_is_let_go_when_its_map_closes() {
    let t = Temp::new("hs-close");
    let mut app = open(&t);
    app.script_working_copy(None).unwrap();
    app.scripts.external.as_mut().unwrap().map = "invented_other".into();
    assert!(app.check_working_copy(Instant::now(), true));
    assert!(app.scripts.external.is_none());
}

#[test]
fn file_names_and_changes_read_plainly() {
    assert_eq!(file_stem("invented map/1"), "invented_map_1");
    assert_eq!(file_stem(""), "map");
    assert_eq!(file_stem(".hidden"), "map.hidden");
    assert_eq!(
        working_path(Path::new("/d"), "a b"),
        Path::new("/d/scripts/a_b/a_b.hsc")
    );
    let c = Changes {
        changed: vec!["main".into(), "helper".into()],
        added: vec!["intro".into()],
        removed: vec![],
    };
    assert_eq!(c.describe(), "main, helper edited; intro added");
    assert_eq!(c.count(), 3);
    assert!(Changes::default().is_empty());
}
