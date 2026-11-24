//! `asset.*` over an invented tags tree of two maps and one local package,
//! `kit-test` (docs/editor/studio-redesign.md, "The browser"): no game
//! data. The tree's files hold placeholder bytes, which the browser never
//! decodes. The baked packages (`pogo`) are in the set too, so a count of
//! every row is taken of a package or a map, not of the whole tree.

use super::*;
use map::FourCC;
use ::tags::repo::store::Manifest;
use ::tags::repo::{TagKey, TagRepo};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 1, 2, 3, 4];

fn key(g: &[u8; 4], path: &str) -> TagKey {
    TagKey::new(FourCC(*g), path)
}

/// Two maps: `alpha` and `beta` share the rifle, hold different bitmaps
/// named `objects\pixels`, and each has its scenario.
pub(super) fn tree(root: &std::path::Path) {
    let mut repo = TagRepo::create(root).unwrap();
    let rifle = key(b"weap", "objects\\weapons\\rifle");
    let pixels = key(b"bitm", "objects\\pixels");
    let place = |repo: &TagRepo, k: &TagKey, v: Option<&str>| {
        let p = repo.path_of(k, v);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"placeholder").unwrap();
    };
    for k in [&rifle, &pixels] {
        place(&repo, k, None);
    }
    place(&repo, &pixels, Some("v2"));
    let sa = key(b"scnr", "scenarios\\alpha");
    let sb = key(b"scnr", "scenarios\\beta");
    place(&repo, &sa, None);
    place(&repo, &sb, None);
    for (map, scenario, variant) in [("alpha", &sa, None), ("beta", &sb, Some("v2"))] {
        repo.put_manifest(
            &Manifest {
                map: map.to_owned(),
                name: map.to_owned(),
                scenario: Some(scenario.clone()),
                tags: vec![
                    (rifle.clone(), None),
                    (pixels.clone(), variant.map(str::to_owned)),
                    (scenario.clone(), None),
                ],
                ..Default::default()
            },
            b"header",
        )
        .unwrap();
    }
}

/// One local package: adds a weapon for `beta`, edits the rifle and adds
/// a bitmap from a PNG.
pub(super) fn package(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("tags")).unwrap();
    std::fs::create_dir_all(dir.join("edits")).unwrap();
    std::fs::create_dir_all(dir.join("art")).unwrap();
    std::fs::write(
        dir.join("package.toml"),
        r#"[package]
format = 1
id = "kit-test"
name = "Kit test"

[[item]]
kind = "tag"
source = "tags/kit_new.weap.yaml"
tag = "objects\\weapons\\kit_new"
maps = ["beta"]

[[item]]
kind = "edits"
source = "edits/kit.yaml"

[[item]]
kind = "bitmap"
source = "art/icon.png"
tag = "ui\\icon"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("tags/kit_new.weap.yaml"),
        "like: 'weap:objects\\weapons\\rifle'\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("edits/kit.yaml"),
        "- tag: 'weap:objects\\weapons\\rifle'\n  set:\n    - {path: x, value: 1}\n",
    )
    .unwrap();
    std::fs::write(dir.join("art/icon.png"), PNG).unwrap();
}

fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    tree(&app.tags_root());
    package(&t.path("data/packages/kit-test"));
    (t, app, Api::new())
}

fn rows(v: &Value) -> Vec<Value> {
    v["rows"].as_array().unwrap().clone()
}

fn paths(v: &Value) -> Vec<String> {
    rows(v)
        .iter()
        .map(|r| r["path"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn a_shared_tag_is_one_row_and_a_differing_one_lists_its_variants() {
    let (_t, mut app, mut api) = setup("assets-variants");
    let v = call(&mut api, &mut app, m::ASSET_LIST, json!({"query": "rifle"}));
    let r = rows(&v);
    assert_eq!(r.len(), 1, "{v}");
    assert_eq!(r[0]["asset"], "tag:weap:objects\\weapons\\rifle");
    assert_eq!(r[0]["kind"], "Objects");
    assert_eq!(r[0]["maps"], json!(["alpha", "beta"]));
    assert_eq!(r[0]["variants"], json!([]));
    // The package's edit line touches it: its badge says so.
    assert_eq!(r[0]["badge"], "edited by kit-test");
    assert_eq!(r[0]["packages"], json!(["kit-test"]));

    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"query": "pixels"}),
    );
    let r = rows(&v);
    assert_eq!(r.len(), 1, "{v}");
    assert_eq!(
        r[0]["variants"],
        json!([
            {"variant": null, "maps": ["alpha"]},
            {"variant": "v2", "maps": ["beta"]},
        ])
    );
    assert_eq!(r[0]["badge"], Value::Null);
}

#[test]
fn kind_map_and_query_narrow_the_tree() {
    let (_t, mut app, mut api) = setup("assets-narrow");
    let list = |api: &mut Api, app: &mut App, p: Value| call(api, app, m::ASSET_LIST, p);

    let v = list(&mut api, &mut app, json!({"kind": "Maps"}));
    assert_eq!(paths(&v), ["scenarios\\alpha", "scenarios\\beta"]);
    // A tag group is a kind too.
    let v = list(&mut api, &mut app, json!({"kind": "scnr"}));
    assert_eq!(paths(&v).len(), 2);

    // The Map picker: only what that map holds. The package's weapon is
    // for beta only.
    let v = list(
        &mut api,
        &mut app,
        json!({"map": "alpha", "kind": "Objects"}),
    );
    assert_eq!(paths(&v), ["objects\\weapons\\rifle"]);
    let v = list(
        &mut api,
        &mut app,
        json!({"map": "beta", "kind": "Objects"}),
    );
    assert_eq!(
        paths(&v),
        ["objects\\weapons\\kit_new", "objects\\weapons\\rifle"]
    );
    assert_eq!(v["maps"], json!(["alpha", "beta"]));

    // Every word must match, in any order, with either slash.
    assert_eq!(
        paths(&list(&mut api, &mut app, json!({"query": "weapons/rifle"}))).len(),
        1
    );
    assert_eq!(
        paths(&list(&mut api, &mut app, json!({"query": "rifle objects"}))).len(),
        1
    );
    assert!(paths(&list(&mut api, &mut app, json!({"query": "rifle nothing"}))).is_empty());
    // The package id is a word of its tags.
    let v = list(&mut api, &mut app, json!({"query": "kit-test weap"}));
    assert_eq!(
        paths(&v),
        ["objects\\weapons\\kit_new", "objects\\weapons\\rifle"]
    );

    // A package's own filter, and the limit.
    let v = list(&mut api, &mut app, json!({"package": "kit-test"}));
    assert_eq!(v["total"], 3);
    let v = list(&mut api, &mut app, json!({"package": "kit-test", "limit": 1}));
    assert_eq!(
        (v["total"].clone(), v["truncated"].clone()),
        (json!(3), json!(true))
    );
    assert_eq!(rows(&v).len(), 1);
}

#[test]
fn a_package_tag_is_new_in_its_package() {
    let (_t, mut app, mut api) = setup("assets-new");
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"query": "kit_new"}),
    );
    let r = rows(&v);
    assert_eq!(r.len(), 1, "{v}");
    assert_eq!(r[0]["new"], true);
    assert_eq!(r[0]["origin"], "package");
    assert_eq!(r[0]["badge"], "new in kit-test");
    assert_eq!(r[0]["maps"], json!([]));
}

#[test]
fn the_packages_root_lists_the_set_and_a_package_its_items() {
    let (_t, mut app, mut api) = setup("assets-packages");
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"root": "packages"}),
    );
    let kit = rows(&v)
        .into_iter()
        .find(|r| r["id"] == "kit-test")
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(kit["asset"], "package:kit-test");
    assert_eq!(kit["on"], true);
    assert_eq!(kit["origin"], "local");

    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"root": "packages", "package": "kit-test"}),
    );
    let kinds: Vec<_> = rows(&v).iter().map(|r| r["kind"].clone()).collect();
    assert_eq!(kinds, [json!("tag"), json!("edits"), json!("bitmap")]);
    assert_eq!(rows(&v)[0]["asset"], "item:kit-test:0");

    let e = api
        .call(
            &mut app,
            m::ASSET_LIST,
            json!({"root": "packages", "package": "nope"}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    let e = api
        .call(&mut app, m::ASSET_LIST, json!({"root": "nowhere"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    // Live: none held offline.
    let v = call(&mut api, &mut app, m::ASSET_LIST, json!({"root": "live"}));
    assert_eq!(v["offline"], true);
}

#[test]
fn a_tree_that_is_not_there_is_refused_with_the_way_to_make_it() {
    let t = Temp::new("assets-no-tree");
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let e = Api::new()
        .call(&mut app, m::ASSET_LIST, json!({}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("workspace.extract"), "{}", e.message);
}

#[test]
fn copy_into_writes_a_like_item_with_no_bytes_of_the_source() {
    let (t, mut app, mut api) = setup("assets-copy");
    let dir = t.path("data/packages/kit-test");
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_COPY,
        json!({"asset": "tag:weap:objects\\weapons\\rifle", "package": "kit-test"}),
    );
    assert_eq!(v["tag"], "tag:weap:objects\\weapons\\rifle_copy");
    assert_eq!(v["source"], "tags/objects/weapons/rifle_copy.weap.yaml");
    assert_eq!(v["item"], 3);
    assert_eq!(v["doc"], "item:kit-test:3");
    let doc = app.front_doc().unwrap();
    let tab = app.tabs().iter().find(|tab| tab.doc == doc).unwrap();
    assert_eq!(app.tab_json(tab)["badge"], "new in kit-test");
    // The source file names the tag and holds nothing else.
    let text =
        std::fs::read_to_string(dir.join("tags/objects/weapons/rifle_copy.weap.yaml")).unwrap();
    assert_eq!(text, "like: 'weap:objects\\weapons\\rifle'\n");
    // The manifest has the item, and the package still loads with a
    // lock that names the new file.
    let manifest = content::package::Manifest::from_toml(
        &std::fs::read_to_string(dir.join("package.toml")).unwrap(),
    )
    .unwrap();
    let item = &manifest.items[3];
    assert_eq!(item.kind, content::package::Kind::Tag);
    assert_eq!(item.tag.as_deref(), Some("objects\\weapons\\rifle_copy"));
    let lock = std::fs::read_to_string(dir.join("package.lock")).unwrap();
    assert!(lock.contains("rifle_copy.weap.yaml"), "{lock}");
    // The browser lists it as new in the package.
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"query": "rifle_copy"}),
    );
    assert_eq!(rows(&v)[0]["badge"], "new in kit-test");

    // The same copy again is refused, as is a tag the tree lacks, a
    // package that is not there and a malformed asset.
    let again = json!({"asset": "tag:weap:objects\\weapons\\rifle", "package": "kit-test"});
    assert_eq!(
        code_of(api.call(&mut app, m::ASSET_COPY, again)),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::ASSET_COPY,
            json!({"asset": "tag:weap:objects\\nothing", "package": "kit-test"})
        )),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::ASSET_COPY,
            json!({"asset": "tag:weap:objects\\weapons\\rifle", "package": "nope"})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::ASSET_COPY,
            json!({"asset": "nonsense", "package": "kit-test"})
        )),
        code::INVALID_PARAMS
    );
    // An explicit target path.
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_COPY,
        json!({"asset": "tag:weap:objects\\weapons\\rifle", "package": "kit-test", "to": "mine/rifle2"}),
    );
    assert_eq!(v["tag"], "tag:weap:mine\\rifle2");
}

#[test]
fn a_thumbnail_is_a_packages_png_and_a_tag_has_none_yet() {
    let (t, mut app, mut api) = setup("assets-thumb");
    let out = t.path("thumb.png");
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_THUMB,
        json!({"asset": "item:kit-test:2", "out": out.display().to_string()}),
    );
    assert_eq!(v["path"], json!(out.display().to_string()));
    assert_eq!(std::fs::read(&out).unwrap(), PNG);
    // An item that is not a PNG bitmap, and a tag: not drawn yet.
    for asset in ["item:kit-test:0", "tag:weap:objects\\weapons\\rifle"] {
        let e = api
            .call(&mut app, m::ASSET_THUMB, json!({"asset": asset}))
            .unwrap_err();
        assert_eq!(e.code, code::NOT_YET, "{asset}");
    }
}
