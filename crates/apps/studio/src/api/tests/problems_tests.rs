//! `problems.list` over invented packages (no game data).

use super::*;

fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn problems_list_answers_package_and_build_problems() {
    let t = Temp::new("problems-list");
    let mut app = app(&t);
    let mut api = Api::new();
    let bad = t.path("data/packages/bad");
    write(
        &bad.join("package.toml"),
        "[package]\nformat = 1\nid = \"bad\"\nname = \"Bad\"\n\n[[item]]\nkind = \"bitmap\"\nsource = \"art/bad.png\"\ntag = \"ui\\\\bad\"\n",
    );
    write(&bad.join("art/bad.png"), "not a picture");

    let v = call(&mut api, &mut app, m::PROBLEMS_LIST, json!({"source": "package"}));
    let rows = v["problems"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["source"], "package");
    assert_eq!(rows[0]["asset"], "item:bad:0");
    assert_eq!(rows[0]["severity"], "error");
    assert_eq!(v["counts"]["error"], 1);
    assert_eq!(v["counts"]["by_source"]["package"], 1);
    assert_eq!(v["ran"][0]["check"], "package.check");
    assert_eq!(v["skipped"], json!([]));
    // The panel reads the same list the call answered.
    assert_eq!(app.problems.rows.len(), 1);

    // Another source's read leaves the package row in the panel's list.
    let b = call(&mut api, &mut app, m::PROBLEMS_LIST, json!({"source": "build"}));
    assert_eq!(b["problems"], json!([]));
    assert_eq!(app.problems.rows.len(), 1);

    assert_eq!(
        code_of(api.call(&mut app, m::PROBLEMS_LIST, json!({"source": "nowhere"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn problems_list_and_check_run_narrow_to_one_asset() {
    let t = Temp::new("problems-asset");
    let mut app = app(&t);
    let mut api = Api::new();
    let bad = t.path("data/packages/bad");
    write(
        &bad.join("package.toml"),
        "[package]\nformat = 1\nid = \"bad\"\nname = \"Bad\"\n\n[[item]]\nkind = \"bitmap\"\nsource = \"art/bad.png\"\ntag = \"ui\\\\bad\"\n",
    );
    write(&bad.join("art/bad.png"), "not a picture");

    // The list narrowed to the item is the item's one row; to another, none.
    let one = call(&mut api, &mut app, m::PROBLEMS_LIST, json!({"asset": "item:bad:0"}));
    assert_eq!(one["problems"].as_array().unwrap().len(), 1);
    assert_eq!(one["counts"]["total"], 1);
    let checks: Vec<_> = one["ran"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["check"].as_str().unwrap())
        .collect();
    assert_eq!(checks, ["package.check", "package.diff"]);
    let none = call(&mut api, &mut app, m::PROBLEMS_LIST, json!({"asset": "item:bad:1"}));
    assert_eq!(none["problems"], json!([]));

    // check.run says the same findings, and the header's count is the same list.
    let run = call(&mut api, &mut app, m::CHECK_RUN, json!({"asset": "item:bad:0"}));
    assert_eq!(run["asset"], "item:bad:0");
    assert_eq!(run["findings"], one["problems"]);
    assert_eq!(run["ran"], one["ran"]);
    assert_eq!(run["counts"]["total"], 1);
    assert_eq!(app.asset_problems("item:bad:0").len(), 1);

    for method in [m::PROBLEMS_LIST, m::CHECK_RUN] {
        assert_eq!(
            code_of(api.call(&mut app, method, json!({"asset": "nowhere"}))),
            code::INVALID_PARAMS
        );
    }
}
