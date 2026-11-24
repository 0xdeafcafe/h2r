//! `package.*` over invented packages under a temporary data root (no game
//! data): the document's edits, the set file's rules and the checks
//! (docs/editor/agent-api.md, "Packages").

use super::*;

fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    (t, app, Api::new())
}

fn new_package(api: &mut Api, app: &mut App, id: &str) {
    call(api, app, m::PACKAGE_NEW, json!({"id": id, "name": id}));
}

fn require(api: &mut Api, app: &mut App, id: &str, requires: &[&str]) -> Reply {
    api.call(
        app,
        m::PACKAGE_SET,
        json!({"id": id, "path": "requires", "value": requires}),
    )
}

fn ids(v: &Value) -> Vec<String> {
    v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

fn message(r: Reply) -> (i64, String) {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => (e.code, e.message),
    }
}

#[test]
fn a_new_package_is_listed_read_and_locked() {
    let (t, mut app, mut api) = setup("pkg-new");
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_NEW,
        json!({"id": "mine", "name": "Mine"}),
    );
    assert_eq!(v["items"], 0);
    let dir = t.path("data/packages/mine");
    assert!(dir.join("package.toml").is_file());
    assert!(dir.join("package.lock").is_file());

    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    let mine = list["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "mine")
        .unwrap();
    assert_eq!(mine["origin"], "local");
    assert_eq!(mine["editable"], true);
    assert_eq!(mine["on"], true);
    assert_eq!(mine["target"], false);

    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "mine"}));
    assert_eq!(read["package"]["name"], "Mine");
    assert_eq!(read["items"], json!([]));
    assert_eq!(read["problems"], json!([]));

    // An id that is taken, malformed or reserved is refused.
    for (id, code) in [
        ("mine", code::REFUSED),
        ("Mine!", code::INVALID_PARAMS),
        ("core", code::REFUSED),
        ("ui", code::REFUSED),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::PACKAGE_NEW, json!({"id": id, "name": "x"}))),
            code,
            "{id}"
        );
    }
}

#[test]
fn set_edits_the_header_and_refuses_what_would_not_read() {
    let (t, mut app, mut api) = setup("pkg-set");
    new_package(&mut api, &mut app, "mine");
    call(
        &mut api,
        &mut app,
        m::PACKAGE_SET,
        json!({"id": "mine", "path": "credit.authors", "value": ["Someone"]}),
    );
    call(
        &mut api,
        &mut app,
        m::PACKAGE_SET,
        json!({"id": "mine", "path": "description", "value": "Words."}),
    );
    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "mine"}));
    assert_eq!(read["credit"]["authors"], json!(["Someone"]));
    assert_eq!(read["package"]["description"], "Words.");
    // A missing value removes an optional field.
    call(
        &mut api,
        &mut app,
        m::PACKAGE_SET,
        json!({"id": "mine", "path": "description"}),
    );
    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "mine"}));
    assert!(read["package"].get("description").is_none(), "{read}");

    let before = std::fs::read_to_string(t.path("data/packages/mine/package.toml")).unwrap();
    for (path, value) in [
        ("id", json!("other")),
        ("format", json!(2)),
        ("nonsense", json!(1)),
        ("name", json!(7)),
        ("name", json!("  ")),
        ("requires", json!(["Not A Slug"])),
        ("requires", json!(["mine"])),
        ("on.the.table", json!(1)),
    ] {
        let r = api.call(
            &mut app,
            m::PACKAGE_SET,
            json!({"id": "mine", "path": path, "value": value}),
        );
        assert!(r.is_err(), "{path} {value}");
    }
    assert_eq!(
        std::fs::read_to_string(t.path("data/packages/mine/package.toml")).unwrap(),
        before
    );
    // A built-in package is read, never written.
    let (c, why) = message(api.call(
        &mut app,
        m::PACKAGE_SET,
        json!({"id": "pogo", "path": "name", "value": "Mine"}),
    ));
    assert_eq!(c, code::REFUSED);
    assert!(why.contains("built in"), "{why}");
}

#[test]
fn a_cycle_through_requires_is_refused_and_the_manifest_stays() {
    let (t, mut app, mut api) = setup("pkg-cycle");
    new_package(&mut api, &mut app, "a");
    new_package(&mut api, &mut app, "b");
    require(&mut api, &mut app, "b", &["a"]).unwrap();
    let before = std::fs::read_to_string(t.path("data/packages/a/package.toml")).unwrap();
    let (c, why) = message(require(&mut api, &mut app, "a", &["b"]));
    assert_eq!(c, code::REFUSED);
    assert!(why.contains("cycle"), "{why}");
    assert_eq!(
        std::fs::read_to_string(t.path("data/packages/a/package.toml")).unwrap(),
        before
    );
}

#[test]
fn always_on_packages_cannot_be_switched_off_by_a_call_or_a_set_file() {
    let (t, mut app, mut api) = setup("pkg-always");
    for id in ["core", "ui"] {
        let (c, why) =
            message(api.call(&mut app, m::PACKAGE_ENABLE, json!({"id": id, "on": false})));
        assert_eq!(c, code::REFUSED);
        assert!(
            why.contains(&format!("{id:?}")) && why.contains("always on"),
            "{why}"
        );
    }
    assert!(!t.path("data/packages/set.toml").exists());

    // A set.toml written by hand that does the same is named by the list.
    std::fs::create_dir_all(t.path("data/packages")).unwrap();
    std::fs::write(t.path("data/packages/set.toml"), "off = [\"core\"]\n").unwrap();
    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    let problems = list["problems"].to_string();
    assert!(
        problems.contains("core") && problems.contains("always on"),
        "{problems}"
    );
    // The same file stops a write that would build on it.
    let (c, why) = message(api.call(&mut app, m::PACKAGE_ORDER, json!({"ids": ["pogo"]})));
    assert_eq!(c, code::REFUSED);
    assert!(why.contains("core"), "{why}");
}

#[test]
fn enable_writes_the_set_file_and_names_what_it_leaves_out() {
    let (t, mut app, mut api) = setup("pkg-enable");
    new_package(&mut api, &mut app, "a");
    new_package(&mut api, &mut app, "b");
    require(&mut api, &mut app, "b", &["a"]).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_ENABLE,
        json!({"id": "a", "on": false}),
    );
    assert_eq!(v["off"], json!(["a"]));
    assert_eq!(v["left_out"][0]["package"], "b");
    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    let row = |id: &str| {
        list["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(row("a")["on"], false);
    assert_eq!(row("b")["on"], false);
    assert!(row("b")["why"].as_str().unwrap().contains("a"));
    let text = std::fs::read_to_string(t.path("data/packages/set.toml")).unwrap();
    assert!(text.contains("\"a\""), "{text}");

    call(
        &mut api,
        &mut app,
        m::PACKAGE_ENABLE,
        json!({"id": "a", "on": true}),
    );
    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    assert!(
        list["packages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != "a" || p["on"] == true)
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::PACKAGE_ENABLE,
            json!({"id": "nothing", "on": true})
        )),
        code::INVALID_PARAMS
    );
}

#[test]
fn an_order_that_breaks_requires_is_refused_naming_both() {
    let (t, mut app, mut api) = setup("pkg-order");
    new_package(&mut api, &mut app, "a");
    new_package(&mut api, &mut app, "b");
    new_package(&mut api, &mut app, "c");
    require(&mut api, &mut app, "b", &["a"]).unwrap();

    let (c, why) = message(api.call(&mut app, m::PACKAGE_ORDER, json!({"ids": ["b", "a"]})));
    assert_eq!(c, code::REFUSED);
    assert!(why.contains("\"b\"") && why.contains("\"a\""), "{why}");
    assert!(!t.path("data/packages/set.toml").exists());

    // Unknown and repeated ids are wrong params.
    for ids in [json!(["a", "nothing"]), json!(["a", "a"])] {
        assert_eq!(
            code_of(api.call(&mut app, m::PACKAGE_ORDER, json!({"ids": ids}))),
            code::INVALID_PARAMS
        );
    }

    // A good order is written and is the load order of the independent ones.
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_ORDER,
        json!({"ids": ["c", "a", "b"]}),
    );
    assert_eq!(v["order"], json!(["c", "a", "b"]));
    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    let all = ids(&list);
    let at = |id: &str| all.iter().position(|i| i == id).unwrap();
    assert!(at("c") < at("a") && at("a") < at("b"), "{all:?}");
}

#[test]
fn check_names_a_cache_map_a_bundle_and_a_dataset_folder() {
    let (t, mut app, mut api) = setup("pkg-check");
    new_package(&mut api, &mut app, "bad");
    let dir = t.path("data/packages/bad");
    std::fs::write(dir.join("lockout.map"), b"x").unwrap();
    std::fs::write(dir.join("pack.h2tag"), b"x").unwrap();
    std::fs::create_dir_all(dir.join("voice/dataset")).unwrap();
    std::fs::write(dir.join("voice/dataset/a.wav"), b"x").unwrap();

    let v = call(&mut api, &mut app, m::PACKAGE_CHECK, json!({"id": "bad"}));
    assert_eq!(v["ok"], false);
    let text = v["findings"].to_string();
    for want in [
        "lockout.map: a cache map",
        "pack.h2tag: a .h2tag bundle",
        "voice/dataset: a voice dataset or model weights folder",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    // Read says the same.
    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "bad"}));
    assert!(read["problems"].to_string().contains("a cache map"));

    // A clean package is ok.
    new_package(&mut api, &mut app, "clean");
    let v = call(&mut api, &mut app, m::PACKAGE_CHECK, json!({"id": "clean"}));
    assert_eq!(v["ok"], true, "{v}");
}

#[test]
fn check_finds_two_packages_adding_one_tag() {
    let (t, mut app, mut api) = setup("pkg-conflict");
    for id in ["one", "two"] {
        new_package(&mut api, &mut app, id);
        let dir = t.path(&format!("data/packages/{id}"));
        std::fs::write(dir.join("t.jpt!.yaml"), "fields: {}\n").unwrap();
        call(
            &mut api,
            &mut app,
            m::PACKAGE_ITEM_ADD,
            json!({"id": id, "item": {"kind": "tag", "source": "t.jpt!.yaml", "tag": "invented\\dmg"}}),
        );
    }
    let v = call(&mut api, &mut app, m::PACKAGE_CHECK, json!({"id": "two"}));
    assert!(v["findings"].to_string().contains("adds tag"), "{v}");
    assert_eq!(v["findings"][0]["kind"], "conflict");
}

#[test]
fn items_are_added_set_and_removed_with_the_lock_following() {
    let (t, mut app, mut api) = setup("pkg-items");
    new_package(&mut api, &mut app, "mine");
    let dir = t.path("data/packages/mine");
    std::fs::write(dir.join("note.txt"), "a note").unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_ITEM_ADD,
        json!({"id": "mine", "item": {"kind": "data", "source": "note.txt"}}),
    );
    assert_eq!(v["item"], 0);
    assert_eq!(v["asset"], "item:mine:0");
    assert!(
        std::fs::read_to_string(dir.join("package.lock"))
            .unwrap()
            .contains("note.txt")
    );

    call(
        &mut api,
        &mut app,
        m::PACKAGE_ITEM_SET,
        json!({"id": "mine", "index": 0, "path": "maps", "value": ["lockout"]}),
    );
    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "mine"}));
    assert_eq!(read["items"][0]["maps"], json!(["lockout"]));
    assert_eq!(read["items"][0]["builder"], "data");
    assert_eq!(read["items"][0]["builds"], json!([]));
    assert_eq!(read["items"][0]["changes_map"], false);

    // Refused: a file that is not there, a path out of the folder, an
    // unknown key, an index past the end. The manifest does not change.
    let before = std::fs::read_to_string(dir.join("package.toml")).unwrap();
    for params in [
        json!({"id": "mine", "item": {"kind": "data", "source": "gone.txt"}}),
        json!({"id": "mine", "item": {"kind": "data", "source": "../note.txt"}}),
        json!({"id": "mine", "item": {"kind": "data", "source": "note.txt", "colour": 1}}),
        json!({"id": "mine", "item": {"kind": "data", "source": "note.txt"}, "at": 9}),
        json!({"id": "mine", "item": {"kind": "map"}}),
    ] {
        assert!(
            api.call(&mut app, m::PACKAGE_ITEM_ADD, params.clone())
                .is_err(),
            "{params}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("package.toml")).unwrap(),
        before
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::PACKAGE_ITEM_REMOVE,
            json!({"id": "mine", "index": 4})
        )),
        code::INVALID_PARAMS
    );

    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_ITEM_REMOVE,
        json!({"id": "mine", "index": 0}),
    );
    assert_eq!(v["items"], 0);
    assert!(dir.join("note.txt").is_file());
    assert!(
        !std::fs::read_to_string(dir.join("package.lock"))
            .unwrap()
            .contains("note.txt")
    );
}

#[test]
fn the_edit_target_is_read_set_cleared_and_the_default_id() {
    let (t, mut app, mut api) = setup("pkg-target");
    new_package(&mut api, &mut app, "mine");
    std::fs::write(t.path("data/packages/mine/note.txt"), "x").unwrap();
    let item = json!({"item": {"kind": "data", "source": "note.txt"}});

    let v = call(&mut api, &mut app, m::PACKAGE_TARGET, json!({}));
    assert_eq!(v["target"], Value::Null);
    // With no id and no target, a write answers NO_TARGET.
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_ITEM_ADD, item.clone())),
        code::NO_TARGET
    );
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_CHECK, json!({}))),
        code::NO_TARGET
    );
    // A built-in package and one that is not there cannot be the target.
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_TARGET, json!({"id": "pogo"}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_TARGET, json!({"id": "nothing"}))),
        code::INVALID_PARAMS
    );

    let v = call(&mut api, &mut app, m::PACKAGE_TARGET, json!({"id": "mine"}));
    assert_eq!(v["target"], "mine");
    let list = call(&mut api, &mut app, m::PACKAGE_LIST, json!({}));
    assert_eq!(list["target"], "mine");
    call(&mut api, &mut app, m::PACKAGE_ITEM_ADD, item);
    let read = call(&mut api, &mut app, m::PACKAGE_READ, json!({"id": "mine"}));
    assert_eq!(read["items"].as_array().unwrap().len(), 1);
    assert_eq!(read["target"], true);

    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_TARGET,
        json!({"clear": true}),
    );
    assert_eq!(v["target"], Value::Null);
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_CHECK, json!({}))),
        code::NO_TARGET
    );
}

#[test]
fn build_and_play_need_a_source_and_a_pairing() {
    let (_t, mut app, mut api) = setup("pkg-build");
    new_package(&mut api, &mut app, "mine");
    // No source is open.
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_BUILD, json!({"id": "mine"}))),
        code::NO_DOCUMENT
    );
    // No pairing.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::PACKAGE_PLAY,
            json!({"id": "mine", "map": "example"})
        )),
        code::REFUSED
    );
}

#[test]
fn build_reports_a_row_per_map_the_package_touches() {
    let t = Temp::new("pkg-build-map");
    let mut app = app(&t);
    let mut api = Api::new();
    new_package(&mut api, &mut app, "mine");
    let dir = t.path("data/packages/mine");
    // A package with nothing for the map: the named map answers untouched.
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_BUILD,
        json!({"id": "mine", "map": "example"}),
    );
    assert_eq!(v["maps"][0]["map"], "example");
    assert_eq!(v["maps"][0]["touched"], false);
    // With no map named, a map it does not touch is left out.
    let v = call(&mut api, &mut app, m::PACKAGE_BUILD, json!({"id": "mine"}));
    assert_eq!(v["maps"], json!([]));

    // An item for the map: a row, built or with its problem named.
    std::fs::write(dir.join("t.jpt!.yaml"), "fields: {}\n").unwrap();
    call(
        &mut api,
        &mut app,
        m::PACKAGE_ITEM_ADD,
        json!({"id": "mine", "item": {"kind": "tag", "source": "t.jpt!.yaml", "tag": "invented\\dmg"}}),
    );
    let v = call(&mut api, &mut app, m::PACKAGE_BUILD, json!({"id": "mine"}));
    let row = &v["maps"][0];
    assert_eq!(row["map"], "example");
    assert_eq!(row["touched"], true);
    assert!(row["took_ms"].is_u64(), "{row}");
    assert!(row["problems"].is_array());

    // A package that is off is refused with why.
    call(
        &mut api,
        &mut app,
        m::PACKAGE_ENABLE,
        json!({"id": "mine", "on": false}),
    );
    let (c, why) = message(api.call(&mut app, m::PACKAGE_BUILD, json!({"id": "mine"})));
    assert_eq!(c, code::REFUSED);
    assert!(why.contains("off"), "{why}");
}
