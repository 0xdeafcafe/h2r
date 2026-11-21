//! The Scripts tool over the scenario crate's invented map with invented
//! script tables and an invented executable (`hs::fixture`). No game data.
use std::path::{Path, PathBuf};

use hs::Item;
use hs::fixture::{Builtin, Exe, Tables};
use scenario_edit::{BlockKind, fixture};

use super::*;
use crate::app::map_cards_tests::Temp;
use crate::app::{Action, App, ToolKind};

/// The invented value types, at the indices the reader dispatches on.
pub(crate) fn types() -> Vec<String> {
    let mut types: Vec<String> = (0..62).map(|i| format!("invented_{i}")).collect();
    for (i, n) in [
        (2, "function_name"),
        (4, "void"),
        (5, "boolean"),
        (6, "real"),
        (7, "short"),
        (9, "string"),
        (20, "trigger_volume"),
        (30, "invented_weapon"),
        (56, "object_name"),
    ] {
        types[i] = n.to_owned();
    }
    types
}

/// The invented executable: builtins 1 to 3 are called by the scripts
/// below, 4 and 5 by none, 6 takes its own arguments.
pub(crate) fn exe() -> Exe {
    let b = |returns, parameters: &[u16]| Builtin {
        returns,
        parameters: parameters.to_vec(),
        ..Builtin::default()
    };
    Exe {
        types: types(),
        builtins: vec![
            b(4, &[]),
            b(4, &[56]),
            b(5, &[20]),
            b(4, &[30]),
            b(4, &[7]),
            b(4, &[6]),
            Builtin {
                returns: 4,
                flags: 2,
                ..Builtin::default()
            },
        ],
        ..Exe::default()
    }
}

/// A global `count`, a static script `helper` naming object `door`, and
/// `main` calling it, watching volume `zone`, naming the weapon tag and
/// reading `count`.
pub(crate) fn tables() -> Tables {
    let mut t = Tables::new();
    let three = t.value(7, 3);
    t.global_definition("count", 7, three);
    let door = t.literal(56, "door", 0);
    let root = t.call(1, "invented_select", 4, &[door]);
    t.script("helper", 3, 4, root);
    let helper = t.script_call(0, "helper", 4, &[]);
    let zone = t.literal(20, "zone", 0);
    let watch = t.call(2, "invented_watch", 5, &[zone]);
    let weapon = t.literal(30, "objects/test/blaster", fixture::id(2).0);
    let spawn = t.call(3, "invented_spawn", 4, &[weapon]);
    let count = t.global(0, "count", 7);
    let root = t.call(6, "invented_all", 4, &[helper, watch, spawn, count]);
    t.script("main", 0, 4, root);
    t.reference(*b"weap", fixture::id(2).0);
    t.free();
    t.free();
    t
}

/// The invented map with the scripts and the object name `door` (weapon
/// 1), at `dir/example.map`, with the invented `default.xbe` beside it
/// when `executable`.
pub(crate) fn scripted_source(dir: &Path, executable: bool) -> PathBuf {
    let mut b = fixture::fixture();
    let name = fixture::sfile(fixture::NAMES_AT);
    b[name..name + 5].copy_from_slice(b"door\0");
    let payload = &mut b[fixture::sfile(0)..fixture::sfile(fixture::SCNR_SIZE)];
    tables().write(payload, fixture::META + fixture::SCNR as u32, 0xa00);
    let map = dir.join("example.map");
    std::fs::write(&map, b).unwrap();
    if executable {
        std::fs::write(dir.join("default.xbe"), exe().xbe()).unwrap();
    }
    map
}

/// An invented documentation listing: three entries agree with the
/// scripts' names, one fits builtin 4, and none fits builtin 5.
pub(crate) fn listing(dir: &Path) -> PathBuf {
    let p = dir.join("invented_doc.txt");
    std::fs::write(
        &p,
        "; invented functions\n\n(<void> invented_select <object_name>)\npicks\n\n\
         (<boolean> invented_watch <trigger_volume>)\n\n(<void> invented_spawn <invented_weapon>)\n\n\
         (<void> invented_wait <short>)\nwaits a while\n\n(<void> invented_other <boolean>)\n",
    )
    .unwrap();
    p
}

fn open(t: &Temp, executable: bool) -> App {
    let map = scripted_source(&t.0, executable);
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    app.dispatch(Action::SetTool(ToolKind::Scripts));
    app
}

fn loaded(app: &App) -> &Loaded {
    match &app.scripts.state {
        Some(Ok(l)) => l,
        other => panic!("not loaded: {:?}", other.as_ref().map(|r| r.as_ref().err())),
    }
}

#[test]
fn an_invented_scenarios_scripts_list_and_decompile() {
    let t = Temp::new("scripts-list");
    let app = open(&t, true);
    let l = loaded(&app);
    let rows: Vec<_> = l
        .entries
        .iter()
        .map(|e| {
            (
                e.source.item,
                e.source.name.as_str(),
                e.kind.as_str(),
                e.type_name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Item::Global(0), "count", "global", "short"),
            (Item::Script(0), "helper", "static", "void"),
            (Item::Script(1), "main", "startup", "void"),
        ]
    );
    let helper = l.entry(Item::Script(0)).unwrap();
    assert_eq!(
        helper.source.source,
        "(script static void helper\n  (invented_select door))\n"
    );
    // The call and its name, and the literal.
    assert_eq!(helper.nodes, 3);
    let main = &l.entry(Item::Script(1)).unwrap().source.source;
    for part in [
        "(invented_all",
        "(helper)",
        "(invented_watch zone)",
        "count))",
    ] {
        assert!(main.contains(part), "{part} in {main}");
    }
    assert_eq!(l.free_nodes(), 2);
    assert_eq!(l.find("helper"), Some(Item::Script(0)));
    assert_eq!(l.find("count"), Some(Item::Global(0)));
    let names = |f: &str| {
        l.matching(f)
            .map(|e| e.source.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("DOOR"), ["helper"]);
    assert_eq!(names("count"), ["count", "main"]);
    assert_eq!(
        l.uses(LinkKind::Script, "helper")
            .iter()
            .map(|u| u.1)
            .collect::<Vec<_>>(),
        ["main"]
    );
    assert_eq!(l.uses(LinkKind::Object, "door")[0].1, "helper");
    assert_eq!(
        l.uses(LinkKind::Tag, &format!("{:#010x}", fixture::id(2).0))[0].1,
        "main"
    );
}

#[test]
fn a_link_to_an_object_name_selects_the_placement() {
    let t = Temp::new("scripts-object");
    let mut app = open(&t, true);
    let helper = loaded(&app).entry(Item::Script(0)).unwrap();
    let link = helper
        .links
        .iter()
        .position(|l| l.kind == LinkKind::Object)
        .unwrap();
    assert_eq!(
        &helper.source.source[helper.links[link].start..helper.links[link].end],
        "door"
    );
    app.dispatch(Action::Script(ScriptAction::Follow {
        item: Item::Script(0),
        link,
    }));
    assert_eq!(app.session.selected(), Some((BlockKind::Weapons, 1)));
    assert_eq!(app.tool, ToolKind::Scenario);
}

#[test]
fn script_global_and_tag_links_go_to_their_definitions_and_a_missing_volume_says_so() {
    let t = Temp::new("scripts-links");
    let mut app = open(&t, true);
    let l = loaded(&app);
    let main = l.entry(Item::Script(1)).unwrap();
    let kinds: Vec<_> = main
        .links
        .iter()
        .map(|k| (k.kind, k.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (LinkKind::Script, "helper"),
            (LinkKind::Volume, "zone"),
            (LinkKind::Tag, "objects/test/blaster"),
            (LinkKind::Global, "count"),
        ]
    );
    let s = app.session.scenario();
    assert_eq!(
        l.resolve(&main.links[0], s),
        Ok(Target::Item(Item::Script(0)))
    );
    assert_eq!(
        l.resolve(&main.links[2], s),
        Ok(Target::Tag(fixture::id(2)))
    );
    assert_eq!(
        l.resolve(&main.links[3], s),
        Ok(Target::Item(Item::Global(0)))
    );
    assert_eq!(
        l.resolve(&main.links[1], s),
        Err("no trigger volume is named \"zone\"".to_owned())
    );
    app.dispatch(Action::Script(ScriptAction::Follow {
        item: Item::Script(1),
        link: 0,
    }));
    assert_eq!(app.scripts.selected, Some(Item::Script(0)));
    assert_eq!(app.tool, ToolKind::Scripts);
    app.dispatch(Action::Script(ScriptAction::Follow {
        item: Item::Script(1),
        link: 1,
    }));
    assert!(
        app.status()
            .is_some_and(|s| s.is_error() && s.text().contains("zone"))
    );
}

#[test]
fn without_the_executable_the_tool_refuses_by_name() {
    let t = Temp::new("scripts-no-xbe");
    let app = open(&t, false);
    let Some(Err(why)) = &app.scripts.state else {
        panic!("loaded without an executable");
    };
    assert!(why.starts_with(NO_EXECUTABLE), "{why}");
    assert!(why.contains("default.xbe"), "{why}");
}

#[test]
fn builtins_take_a_listings_names_only_where_the_signature_matches() {
    let t = Temp::new("scripts-doc");
    let mut app = open(&t, true);
    {
        let n = &loaded(&app).naming;
        assert_eq!(n.name(1), Some("invented_select"));
        assert_eq!(n.from(1), Some(hs::NameFrom::Map));
        assert_eq!(n.label(4), "#builtin:4");
    }
    app.dispatch(Action::Script(ScriptAction::SetDoc(Some(listing(&t.0)))));
    let l = loaded(&app);
    assert_eq!(l.naming.name(4), Some("invented_wait"));
    assert!(matches!(l.naming.from(4), Some(hs::NameFrom::Doc(_))));
    // `invented_other` takes a boolean; builtin 5 a real.
    assert_eq!(l.naming.label(5), "#builtin:5");
    assert_eq!(
        l.naming.call_form(&l.catalog, 5).unwrap(),
        "(#builtin:5 <real>)"
    );
    assert_eq!(l.naming.report.as_ref().unwrap().anchors, 3);
    app.dispatch(Action::Script(ScriptAction::SetDoc(None)));
    assert_eq!(loaded(&app).naming.name(4), None);
    // A file that is not a listing is named as such; names stay the map's.
    let bad = t.0.join("notes.txt");
    std::fs::write(&bad, "nothing here\n").unwrap();
    app.dispatch(Action::Script(ScriptAction::SetDoc(Some(bad))));
    let l = loaded(&app);
    assert!(matches!(&l.doc, Some((_, Err(e))) if e.contains("no script documentation entries")));
    assert_eq!(l.naming.name(4), None);
}
