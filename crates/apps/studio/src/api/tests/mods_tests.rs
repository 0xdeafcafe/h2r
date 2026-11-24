//! `asset.fork`, `asset.revert`, `package.diff`, **Changed by** in
//! `asset.links` and the conflict rows in Problems
//! (docs/engine/asset-store.md, "In Studio"; slice AS7), over the invented
//! tags tree and package `kit-test` of `assets_tests` and the invented map
//! `example`. No game data.

use super::*;
use crate::app::mods::Conflict;

const RIFLE: &str = "weap:objects\\weapons\\rifle";
const RIFLE_ID: &str = "tag:weap:objects\\weapons\\rifle";

/// The invented map open, the tags tree of two maps and the package
/// `kit-test`, whose `edits/kit.yaml` (every map) sets `x` of the rifle.
fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let app = app(&t);
    assets_tests::tree(&app.tags_root());
    assets_tests::package(&t.path("data/packages/kit-test"));
    (t, app, Api::new())
}

fn fork_params(map: &str) -> Value {
    json!({"tag": RIFLE, "map": map, "package": "kit-test"})
}

fn read(path: std::path::PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn changed_by(api: &mut Api, app: &mut App) -> Vec<Value> {
    let v = call(api, app, m::ASSET_LINKS, json!({"asset": RIFLE_ID}));
    v["changed_by"].as_array().unwrap().clone()
}

#[test]
fn a_fork_puts_the_lines_in_an_item_for_one_map_and_changed_by_lists_it_after() {
    let (t, mut app, mut api) = setup("mods-fork");
    let v = call(&mut api, &mut app, m::ASSET_FORK, fork_params("alpha"));
    assert_eq!(v["file"], "edits/alpha.yaml");
    assert_eq!(v["lines"], 1);
    assert_eq!(v["created"], true);
    // Item 1 of the package, `edits/kit.yaml`, was the lines' source.
    assert_eq!(v["from_items"], json!([1]));
    let dir = t.path("data/packages/kit-test");
    let text = read(dir.join("edits/alpha.yaml"));
    assert!(text.contains("path: x"), "{text}");
    // The item names alpha alone; the one it copied from still has no scope.
    let r = call(
        &mut api,
        &mut app,
        m::PACKAGE_READ,
        json!({"id": "kit-test"}),
    );
    let items = r["items"].as_array().unwrap();
    let forked = items
        .iter()
        .find(|i| i["source"] == "edits/alpha.yaml")
        .expect("the fork's item");
    assert_eq!(forked["maps"], json!(["alpha"]));
    assert_eq!(v["item"], forked["index"]);
    assert_eq!(items[1]["source"], "edits/kit.yaml");
    assert_eq!(items[1]["maps"], "all");

    // Changed by: both packages' items in order, the fork's value wins.
    let by = changed_by(&mut api, &mut app);
    assert_eq!(by.len(), 2, "{by:?}");
    assert_eq!(by[0]["package"], "kit-test");
    assert_eq!(by[0]["item"], 1);
    assert_eq!(by[0]["forked"], false);
    assert_eq!(by[0]["fields"][0]["path"], "x");
    assert_eq!(by[0]["fields"][0]["wins"], false);
    assert_eq!(by[1]["maps"], "alpha");
    assert_eq!(by[1]["forked"], true);
    assert_eq!(by[1]["fields"][0]["value"], 1);
    assert_eq!(by[1]["fields"][0]["wins"], true);
}

#[test]
fn a_tag_forked_already_is_refused_and_set_puts_lines_in_by_path() {
    let (t, mut app, mut api) = setup("mods-fork-twice");
    call(&mut api, &mut app, m::ASSET_FORK, fork_params("alpha"));
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, fork_params("alpha"))),
        code::REFUSED
    );
    let mut with = fork_params("alpha");
    with["set"] = json!([{"path": "y", "value": 2, "from": 0}, {"path": "x", "value": 5}]);
    let v = call(&mut api, &mut app, m::ASSET_FORK, with);
    assert_eq!(v["lines"], 2);
    assert_eq!(v["created"], false);
    let text = read(t.path("data/packages/kit-test/edits/alpha.yaml"));
    assert!(
        text.contains("path: y") && text.contains("from: 0"),
        "{text}"
    );
    // `x` was put in by path: one line, the new value.
    assert_eq!(text.matches("path: x").count(), 1, "{text}");
    assert!(text.contains("value: 5"), "{text}");
}

#[test]
fn a_fork_has_something_to_fork_a_local_package_and_a_valid_map() {
    let (_t, mut app, mut api) = setup("mods-fork-refused");
    // No line for the tag reaches the map: nothing to fork without `set`.
    let other =
        json!({"tag": "weap:objects\\weapons\\other", "map": "alpha", "package": "kit-test"});
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, other.clone())),
        code::REFUSED
    );
    let mut with = other;
    with["set"] = json!([{"path": "z", "value": 3}]);
    let v = call(&mut api, &mut app, m::ASSET_FORK, with);
    assert_eq!(v["lines"], 1);
    // A built-in package is never written.
    let baked = json!({"tag": RIFLE, "map": "alpha", "package": "pogo"});
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, baked)),
        code::REFUSED
    );
    // A map that cannot name a file, and a tag that is not one.
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, fork_params("a/b"))),
        code::INVALID_PARAMS
    );
    let bad = json!({"tag": "nonsense", "map": "alpha", "package": "kit-test"});
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, bad)),
        code::INVALID_PARAMS
    );
    // No package and no edit target.
    let none = json!({"tag": RIFLE, "map": "alpha"});
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_FORK, none)),
        code::NO_TARGET
    );
    // The edit target is the default package.
    call(
        &mut api,
        &mut app,
        m::PACKAGE_TARGET,
        json!({"id": "kit-test"}),
    );
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_FORK,
        json!({"tag": RIFLE, "map": "beta"}),
    );
    assert_eq!(v["package"], "kit-test");
    assert_eq!(v["file"], "edits/beta.yaml");
}

#[test]
fn a_revert_takes_the_fork_out_with_its_item_and_leaves_other_tags_lines() {
    let (t, mut app, mut api) = setup("mods-revert");
    let dir = t.path("data/packages/kit-test");
    call(&mut api, &mut app, m::ASSET_FORK, fork_params("alpha"));
    let other = json!({
        "tag": "weap:objects\\weapons\\other", "map": "alpha", "package": "kit-test",
        "set": [{"path": "z", "value": 3}],
    });
    call(&mut api, &mut app, m::ASSET_FORK, other);
    assert_eq!(changed_by(&mut api, &mut app).len(), 2);

    // The rifle's lines go; the other tag's stay, so the item and file stay.
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_REVERT,
        json!({"tag": RIFLE, "map": "alpha", "package": "kit-test"}),
    );
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert_eq!(v["removed"][0]["file"], "edits/alpha.yaml");
    assert_eq!(v["removed"][0]["item_removed"], false);
    let text = read(dir.join("edits/alpha.yaml"));
    assert!(
        text.contains("path: z") && !text.contains("path: x"),
        "{text}"
    );
    let by = changed_by(&mut api, &mut app);
    assert_eq!(by.len(), 1);
    assert_eq!(by[0]["fields"][0]["wins"], true);
    // The broad item's line was never touched by a revert for the map.
    assert!(read(dir.join("edits/kit.yaml")).contains("path: x"));

    // The last entry of the file goes: the item and the file with it.
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_REVERT,
        json!({"tag": "weap:objects\\weapons\\other", "map": "alpha", "package": "kit-test"}),
    );
    assert_eq!(v["removed"][0]["item_removed"], true);
    assert!(!dir.join("edits/alpha.yaml").exists());
    assert!(!read(dir.join("package.toml")).contains("edits/alpha.yaml"));
    // The package still loads, with a lock that names what it holds.
    let r = call(
        &mut api,
        &mut app,
        m::PACKAGE_CHECK,
        json!({"id": "kit-test"}),
    );
    assert!(
        !r["findings"].to_string().contains("alpha.yaml"),
        "{}",
        r["findings"]
    );
}

#[test]
fn a_revert_without_a_map_takes_every_line_of_the_tag_and_names_what_is_not_there() {
    let (t, mut app, mut api) = setup("mods-revert-all");
    let dir = t.path("data/packages/kit-test");
    call(&mut api, &mut app, m::ASSET_FORK, fork_params("alpha"));
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_REVERT,
        json!({"tag": RIFLE, "package": "kit-test"}),
    );
    // Both the broad item and the fork lose their entry, and both go.
    assert_eq!(v["removed"].as_array().unwrap().len(), 2, "{v}");
    assert!(!dir.join("edits/alpha.yaml").exists());
    assert!(!dir.join("edits/kit.yaml").exists());
    assert_eq!(changed_by(&mut api, &mut app).len(), 0);
    // Nothing left to revert.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::ASSET_REVERT,
            json!({"tag": RIFLE, "package": "kit-test"})
        )),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::ASSET_REVERT,
            json!({"tag": RIFLE, "map": "alpha", "package": "kit-test"})
        )),
        code::REFUSED
    );
}

#[test]
fn a_diff_lists_the_changes_per_asset_and_a_fork_shows_for_its_map_only() {
    let (_t, mut app, mut api) = setup("mods-diff");
    // A fork for a map other than the one open.
    call(&mut api, &mut app, m::ASSET_FORK, fork_params("beta"));

    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_DIFF,
        json!({"id": "kit-test"}),
    );
    assert_eq!(v["id"], "kit-test");
    assert_eq!(v["map"], Value::Null);
    assert_eq!(v["counts"]["edit"], 2, "{v}");
    assert_eq!(v["counts"]["added"], 2);
    assert_eq!(v["counts"]["forked"], 1);
    let assets = v["assets"].as_array().unwrap();
    let edits: Vec<&Value> = assets.iter().filter(|a| a["change"] == "edit").collect();
    assert!(edits.iter().all(|a| a["asset"] == RIFLE_ID));
    assert_eq!(edits[0]["fields"][0]["path"], "x");
    assert_eq!(edits[0]["fields"][0]["to"], 1);
    assert_eq!(edits[0]["maps"], "all");
    assert_eq!(edits[1]["maps"], "beta");
    assert_eq!(edits[1]["forked"], true);
    let added: Vec<&str> = assets
        .iter()
        .filter(|a| a["change"] == "added")
        .map(|a| a["asset"].as_str().unwrap())
        .collect();
    assert!(
        added.contains(&"tag:weap:objects\\weapons\\kit_new"),
        "{added:?}"
    );
    assert!(added.contains(&"tag:bitm:ui\\icon"), "{added:?}");
    // The open map is the only level the broad edit reaches.
    assert_eq!(v["levels"], json!(["example"]));
    assert_eq!(v["built"], false);
    assert_eq!(v["conflicts"], json!([]));

    // For `example`, the fork for beta is not there: one edit row, no fork.
    let v = call(
        &mut api,
        &mut app,
        m::PACKAGE_DIFF,
        json!({"id": "kit-test", "map": "example"}),
    );
    assert_eq!(v["map"], "example");
    assert_eq!(v["counts"]["edit"], 1, "{v}");
    assert_eq!(v["counts"]["forked"], 0);
    // The tag the package adds for beta alone is not for this map either.
    assert_eq!(v["counts"]["added"], 1);
    // The build over the map ran or said why not; rows are always a list.
    assert!(v["built"].is_boolean());
    assert!(v["conflicts"].is_array());
    if v["built"] == false {
        assert!(!v["problems"].as_array().unwrap().is_empty(), "{v}");
    }
}

#[test]
fn a_diff_of_a_package_that_is_not_there_or_a_map_that_is_not_open_is_refused() {
    let (_t, mut app, mut api) = setup("mods-diff-refused");
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_DIFF, json!({"id": "nothing"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::PACKAGE_DIFF,
            json!({"id": "kit-test", "map": "nowhere"})
        )),
        code::INVALID_PARAMS
    );
    // With no id and no edit target.
    assert_eq!(
        code_of(api.call(&mut app, m::PACKAGE_DIFF, json!({}))),
        code::NO_TARGET
    );
    // A baked package reads too, with the rows it has.
    let v = call(&mut api, &mut app, m::PACKAGE_DIFF, json!({"id": "pogo"}));
    assert!(v["assets"].is_array());
}

#[test]
fn conflict_rows_list_in_problems_by_level_and_a_new_read_of_the_level_replaces_them() {
    let (_t, mut app, mut api) = setup("mods-problems");
    let row = |map: &str, kind: &'static str, package: &str| Conflict {
        map: map.to_owned(),
        kind,
        package: package.to_owned(),
        other: (kind == "override").then(|| "kit-test".to_owned()),
        tag: (kind == "override").then(|| RIFLE.to_owned()),
        path: (kind == "override").then(|| "x".to_owned()),
        why: if kind == "refused" {
            "its patch of weap:objects\\weapons\\rifle names a key the map lacks".to_owned()
        } else {
            format!("{RIFLE} x: {package:?} overrides \"kit-test\"")
        },
    };
    app.problems.set_levels(
        "alpha",
        vec![
            row("alpha", "refused", "kit-test"),
            row("alpha", "override", "late"),
        ],
    );
    app.problems
        .set_levels("beta", vec![row("beta", "refused", "late")]);
    let v = call(
        &mut api,
        &mut app,
        m::PROBLEMS_LIST,
        json!({"source": "package"}),
    );
    let rows: Vec<&Value> = v["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| {
            p["code"].as_str().unwrap().starts_with("package.refused")
                || p["code"].as_str().unwrap().starts_with("package.override")
        })
        .collect();
    assert_eq!(rows.len(), 3, "{v}");
    let refused = rows
        .iter()
        .find(|p| p["target"] == "alpha: kit-test")
        .expect("alpha's refusal");
    assert_eq!(refused["severity"], "warning");
    assert_eq!(refused["code"], "package.refused");
    assert_eq!(refused["asset"], "package:kit-test");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("is off for alpha")
    );
    let over = rows
        .iter()
        .find(|p| p["code"] == "package.override")
        .unwrap();
    assert_eq!(over["severity"], "note");
    assert_eq!(over["field"], "x");
    assert!(v["ran"].to_string().contains("package.diff"));

    // Reading alpha again replaces alpha's rows and keeps beta's.
    app.problems.set_levels("alpha", Vec::new());
    let v = call(
        &mut api,
        &mut app,
        m::PROBLEMS_LIST,
        json!({"source": "package"}),
    );
    let left: Vec<&str> = v["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["code"].as_str().unwrap().starts_with("package.refused"))
        .map(|p| p["target"].as_str().unwrap())
        .collect();
    assert_eq!(left, vec!["beta: late"]);
    // Another source's refresh leaves them for the next read of `package`.
    call(
        &mut api,
        &mut app,
        m::PROBLEMS_LIST,
        json!({"source": "build"}),
    );
    assert_eq!(app.problems.levels.len(), 1);
}

#[test]
fn a_package_file_that_is_a_raw_resource_of_the_store_is_a_finding_of_check_and_problems() {
    let (t, mut app, mut api) = setup("mods-store");
    let dir = t.path("data/packages/kit-test");
    let before = call(
        &mut api,
        &mut app,
        m::PACKAGE_CHECK,
        json!({"id": "kit-test"}),
    );
    assert!(!before["findings"].to_string().contains("of the store"));

    // The store holds a raw resource with the icon's bytes.
    let bytes = std::fs::read(dir.join("art/icon.png")).unwrap();
    let hash = content::Sha256::of(&bytes).to_string();
    let at = app.tags_root().join("raw").join(&hash[..2]);
    std::fs::create_dir_all(&at).unwrap();
    std::fs::write(at.join(&hash), &bytes).unwrap();

    let after = call(
        &mut api,
        &mut app,
        m::PACKAGE_CHECK,
        json!({"id": "kit-test"}),
    );
    let found: Vec<&Value> = after["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "store")
        .collect();
    assert_eq!(found.len(), 1, "{after}");
    assert_eq!(found[0]["file"], "art/icon.png");
    assert!(found[0]["message"].as_str().unwrap().contains(&hash));
    // The same row is in package.read and in Problems.
    let r = call(
        &mut api,
        &mut app,
        m::PACKAGE_READ,
        json!({"id": "kit-test"}),
    );
    assert!(r["problems"].to_string().contains("of the store"));
    let v = call(
        &mut api,
        &mut app,
        m::PROBLEMS_LIST,
        json!({"source": "package"}),
    );
    assert!(
        v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["message"].as_str().unwrap().contains("of the store")),
        "{v}"
    );
}

#[test]
fn the_inspectors_fork_and_revert_buttons_act_on_the_open_map_in_the_edit_target() {
    let (t, mut app, mut api) = setup("mods-actions");
    let file = t.path("data/packages/kit-test/edits/example.yaml");
    // With no edit target the button does nothing but say so.
    app.dispatch(Action::ForkAsset {
        asset: RIFLE_ID.to_owned(),
    });
    assert!(!file.exists());
    call(
        &mut api,
        &mut app,
        m::PACKAGE_TARGET,
        json!({"id": "kit-test"}),
    );
    app.dispatch(Action::ForkAsset {
        asset: RIFLE_ID.to_owned(),
    });
    assert!(read(file.clone()).contains("path: x"));
    // Changed by now shows the fork as the one for the open map.
    let by = changed_by(&mut api, &mut app);
    assert_eq!(by.last().unwrap()["maps"], "example");
    assert_eq!(by.last().unwrap()["forked"], true);
    app.dispatch(Action::RevertAsset {
        asset: RIFLE_ID.to_owned(),
    });
    assert!(!file.exists());
    assert_eq!(changed_by(&mut api, &mut app).len(), 1);
}
