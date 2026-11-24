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
