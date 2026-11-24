//! `shell.*` over the invented map: the document tabs as the agent API
//! opens, focuses and closes them, and the same state from the window's
//! commands (docs/editor/studio-redesign.md, "The shell").

use super::*;
use crate::app::Doc;
use crate::app::shell_tests as tabbed;
use crate::app::tag_tests::weap_id;

const KIT: &str = "itmc:objects\\test\\kit";

/// The window's commands and `shell.*` calls, one after the other on two
/// apps, leave the same tabs, front, tools and edits.
#[test]
fn the_window_commands_and_shell_calls_leave_the_same_state() {
    let (ta, tb) = (tabbed::Temp::new("window"), tabbed::Temp::new("api"));
    let mut window = tabbed::app(&ta.0);
    let mut agent = tabbed::app(&tb.0);
    let mut api = Api::new();
    let (weap, kit) = (Doc::Tag(weap_id()), Doc::Tag(tabbed::kit_id()));

    window.dispatch(Action::OpenDoc(weap));
    window.dispatch(Action::OpenDoc(kit));
    window.dispatch(Action::FocusDoc(Doc::Map));
    tabbed::set(&mut window, tabbed::kit_id(), "count", json!(9));
    window.dispatch(Action::UndoDoc(kit));
    window.dispatch(Action::CloseDoc(weap));

    let weap_id_text = format!("{:#010x}", weap_id().0);
    let opened = call(
        &mut api,
        &mut agent,
        m::SHELL_OPEN,
        json!({"tag": weap_id_text}),
    );
    assert_eq!(opened["tab"]["kind"], "tag");
    assert_eq!(opened["tab"]["editor"], "field_panel");
    let weap_doc = opened["tab"]["doc"].as_str().unwrap().to_owned();
    call(&mut api, &mut agent, m::SHELL_OPEN, json!({"tag": "itmc"}));
    call(&mut api, &mut agent, m::SHELL_FOCUS, json!({"doc": "map"}));
    call(
        &mut api,
        &mut agent,
        m::TAG_SET,
        json!({"tag": KIT, "path": "count", "value": 9}),
    );
    let undone = call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": KIT}));
    assert_eq!(
        (undone["done"].clone(), undone["can_redo"].clone()),
        (json!(true), json!(true))
    );
    let closed = call(
        &mut api,
        &mut agent,
        m::SHELL_CLOSE,
        json!({"doc": weap_doc}),
    );
    assert_eq!(closed["closed"], true);

    let tabs = |app: &mut App| call(&mut Api::new(), app, m::SHELL_TABS, Value::Null);
    let (a, b) = (tabs(&mut window), tabs(&mut agent));
    assert_eq!(a, b);
    assert_eq!(a["front"], "map");
    assert_eq!(a["tabs"].as_array().unwrap().len(), 2, "{a}");
    assert_eq!(a["tabs"][1]["doc"], KIT);
    assert_eq!(a["tabs"][1]["can_redo"], true);
    assert_eq!(window.tool, agent.tool);
    let docs = |app: &mut App| call(&mut Api::new(), app, m::DOC_LIST, Value::Null);
    assert_eq!(docs(&mut window), docs(&mut agent));
}

/// What each call answers, the tab's own undo through `doc.*`, and the
/// refusals: an unknown tab, nothing to open, a dirty map closed without
/// discard.
#[test]
fn shell_open_focus_and_close_answer_with_the_tab() {
    let t = Temp::new("shell");
    let mut app = app(&t);
    let mut api = Api::new();
    let tabs = call(&mut api, &mut app, m::SHELL_TABS, Value::Null);
    assert_eq!(tabs["front"], "map");
    assert_eq!(tabs["tabs"][0]["kind"], "map");
    assert_eq!(tabs["tabs"][0]["title"], "example");

    let kit = call(&mut api, &mut app, m::SHELL_OPEN, json!({"tag": "itmc"}));
    assert_eq!(kit["tab"]["doc"], KIT);
    assert_eq!(kit["tab"]["group"], "itmc");
    assert_eq!(kit["tab"]["tool"], "tags");
    assert_eq!(kit["tab"]["map"], "example");
    assert_eq!(kit["front"], KIT);
    // The scenario tag is the map's own tab.
    let scnr = call(&mut api, &mut app, m::SHELL_OPEN, json!({"tag": "scnr"}));
    assert_eq!(
        (scnr["front"].clone(), scnr["tab"]["tool"].clone()),
        (json!("map"), json!("scenario"))
    );
    assert_eq!(
        call(&mut api, &mut app, m::SHELL_TABS, Value::Null)["tabs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    call(&mut api, &mut app, m::SHELL_FOCUS, json!({"doc": KIT}));
    assert_eq!(app.tool, crate::ToolKind::Tags);
    call(
        &mut api,
        &mut app,
        m::TAG_SET,
        json!({"tag": "itmc", "path": "+0x8", "value": 7}),
    );
    let list = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["doc"] == KIT)
        .unwrap()
        .clone();
    assert_eq!(
        (row["can_undo"].clone(), row["dirty"].clone()),
        (json!(true), json!(true))
    );
    let undone = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": KIT}));
    assert_eq!(
        (undone["can_undo"].clone(), undone["can_redo"].clone()),
        (json!(false), json!(true))
    );
    assert!(!app.is_dirty());

    assert_eq!(
        code_of(api.call(&mut app, m::SHELL_FOCUS, json!({"doc": "nope"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::SHELL_OPEN, json!({}))),
        code::INVALID_PARAMS
    );

    // A dirty map asks first; with discard its tab and its tags' close.
    call(
        &mut api,
        &mut app,
        m::TAG_SET,
        json!({"tag": "scnr", "path": "netgame_flags[0].position", "value": [7, 7, 7]}),
    );
    assert_eq!(
        code_of(api.call(&mut app, m::SHELL_CLOSE, json!({"doc": "map"}))),
        code::UNSAVED
    );
    let closed = call(
        &mut api,
        &mut app,
        m::SHELL_CLOSE,
        json!({"doc": "map", "discard": true}),
    );
    assert_eq!(
        (closed["closed"].clone(), closed["front"].clone()),
        (json!(true), Value::Null)
    );
    assert_eq!(
        call(&mut api, &mut app, m::SHELL_TABS, Value::Null)["tabs"],
        json!([])
    );

    // A map and a tag in one call.
    let both = call(
        &mut api,
        &mut app,
        m::SHELL_OPEN,
        json!({"map": "example", "tag": "itmc"}),
    );
    assert_eq!(both["front"], KIT);
    let tabs = call(&mut api, &mut app, m::SHELL_TABS, Value::Null);
    let ids: Vec<&str> = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["doc"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["map", KIT]);
}

/// One local package, `items`, whose items open the editors of what they
/// build: a `rules` script and a `variant` of it (the window fixture
/// `app::tests::items_package` is the same invention).
fn items_app(t: &tabbed::Temp) -> App {
    std::fs::create_dir_all(t.0.join("data/packages/items/scripts")).unwrap();
    std::fs::write(
        t.0.join("data/packages/items/package.toml"),
        r#"[package]
format = 1
id = "items"
name = "Items"

[[item]]
kind = "rules"
source = "scripts/hill.rules"

[[item]]
kind = "variant"
source = "scripts/hill.variant.rules"
"#,
    )
    .unwrap();
    for name in ["hill.rules", "hill.variant.rules"] {
        std::fs::write(
            t.0.join("data/packages/items/scripts").join(name),
            "rules \"Hill\" 1\ntype party\n\nrequires hud.message\n",
        )
        .unwrap();
    }
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app
}

/// `shell.open {asset}` for a package item runs the way in the window's
/// double-click runs ([`Action::OpenAsset`]), so both leave the same
/// tabs, with the item's id as the tab's id
/// (docs/editor/studio-flow.md, "One way in").
#[test]
fn shell_open_of_an_item_leaves_the_windows_tabs() {
    let (ta, tb) = (
        tabbed::Temp::new("items-window"),
        tabbed::Temp::new("items-agent"),
    );
    let mut window = items_app(&ta);
    let mut agent = items_app(&tb);
    let mut api = Api::new();
    for asset in ["item:items:0", "item:items:1"] {
        window.dispatch(Action::OpenAsset {
            asset: asset.to_owned(),
            map: None,
        });
        let opened = call(
            &mut api,
            &mut agent,
            m::SHELL_OPEN,
            json!({ "asset": asset, "discard": true }),
        );
        assert_eq!(opened["tab"]["doc"], asset, "{opened}");
        assert_eq!(opened["front"], asset);
    }
    let tabs = |app: &mut App| call(&mut Api::new(), app, m::SHELL_TABS, Value::Null);
    assert_eq!(tabs(&mut window), tabs(&mut agent), "the two ways agree");
    let tabs = tabs(&mut agent);
    let ids: Vec<&str> = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["doc"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["item:items:0", "item:items:1"]);
    assert_eq!(tabs["front"], "item:items:1");
    assert_eq!(tabs["tabs"][0]["kind"], "rules");

    // The tab the two ways left answers `shell.focus` by the item's id
    // and by the tool document's name.
    call(
        &mut api,
        &mut agent,
        m::SHELL_FOCUS,
        json!({"doc": "rules"}),
    );
    assert_eq!(agent.front_doc(), Some(Doc::Rules));
    call(
        &mut api,
        &mut agent,
        m::SHELL_FOCUS,
        json!({"doc": "item:items:0"}),
    );
    assert_eq!(agent.front_doc(), Some(Doc::Rules));

    // An item the set does not hold is refused by name.
    let why = api
        .call(
            &mut agent,
            m::SHELL_OPEN,
            json!({ "asset": "item:items:9" }),
        )
        .unwrap_err();
    assert_eq!(why.code, code::REFUSED, "{}", why.message);
}
