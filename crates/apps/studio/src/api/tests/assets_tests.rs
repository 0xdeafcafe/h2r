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
    // An item lists under the kind of what it builds: the tag and the
    // edit under the tag kinds, the bitmap under Bitmaps. Its own kind
    // is on the row too.
    let kinds: Vec<_> = rows(&v).iter().map(|r| r["kind"].clone()).collect();
    assert_eq!(
        kinds,
        [json!("Objects"), json!("Objects"), json!("Bitmaps")]
    );
    let item_kinds: Vec<_> = rows(&v).iter().map(|r| r["item_kind"].clone()).collect();
    assert_eq!(item_kinds, [json!("tag"), json!("edits"), json!("bitmap")]);
    assert_eq!(rows(&v)[0]["asset"], "item:kit-test:0");

    let filtered = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"root": "packages", "package": "kit-test", "kind": "bitmaps"}),
    );
    assert_eq!(rows(&filtered).len(), 1);
    assert_eq!(rows(&filtered)[0]["asset"], "item:kit-test:2");

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

/// A tree whose files are real tags, with a catalogue: each content is an
/// asset by hash (and `used` names the keys that reference the tag), until
/// a manifest changes and the catalogue is stale.
#[test]
fn with_a_catalogue_the_rows_carry_assets_and_where_used() {
    let t = Temp::new("assets-catalogue");
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let root = app.tags_root();
    let mut repo = TagRepo::create(&root).unwrap();
    // A tag of one word that names `names` (a datum at +4).
    let tag = |group: &[u8; 4], path: &str, word: u32, names: Option<&TagKey>| {
        let mut bytes = word.to_le_bytes().to_vec();
        bytes.extend([0; 4]);
        let fields = names
            .map(|k| vec![(4, ::tags::repo::Value::Ref(k.clone()))])
            .unwrap_or_default();
        ::tags::repo::LooseTag {
            key: key(group, path),
            body: ::tags::repo::Body::Tree(::tags::repo::Tree {
                nodes: vec![::tags::repo::Node {
                    bytes,
                    count: 1,
                    size: 8,
                    align: 4,
                    fields,
                }],
                ..Default::default()
            }),
        }
    };
    let form = |t: &::tags::repo::LooseTag| {
        let mut tables = ::tags::repo::normal::Tables::none();
        ::tags::repo::normal::form(t, &mut tables).unwrap()
    };
    let rifle = key(b"weap", "objects\\weapons\\rifle");
    let pixels = key(b"bitm", "objects\\pixels");
    let (alpha, beta) = (
        key(b"scnr", "scenarios\\alpha"),
        key(b"scnr", "scenarios\\beta"),
    );
    for t in [
        tag(b"weap", "objects\\weapons\\rifle", 1, Some(&pixels)),
        tag(b"bitm", "objects\\pixels", 1, None),
        tag(b"scnr", "scenarios\\alpha", 3, Some(&rifle)),
    ] {
        repo.put(&t).unwrap();
    }
    // The other map holds the bitmap as another file and its own scenario.
    let stored = repo.put(&tag(b"bitm", "objects\\pixels", 2, None)).unwrap();
    let ::tags::repo::store::Stored::Variant(v2) = stored else {
        panic!("the other bitmap is a variant: {stored:?}")
    };
    repo.put(&tag(b"scnr", "scenarios\\beta", 4, Some(&rifle)))
        .unwrap();
    for (stem, scenario, variant) in [("alpha", &alpha, None), ("beta", &beta, Some(v2.clone()))] {
        repo.put_manifest(
            &Manifest {
                map: stem.into(),
                scenario: Some(scenario.clone()),
                tags: vec![
                    (rifle.clone(), None),
                    (pixels.clone(), variant),
                    (scenario.clone(), None),
                ],
                ..Default::default()
            },
            b"header",
        )
        .unwrap();
    }
    let (c, report) =
        ::tags::repo::catalogue::build(&root, &::tags::schema::Schema::default()).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    ::tags::repo::catalogue::write(&root, &c).unwrap();

    let mut api = Api::new();
    let row = |api: &mut Api, app: &mut App, query: &str| {
        call(api, app, m::ASSET_LIST, json!({"query": query}))["rows"][0].clone()
    };
    // One content: its hash, and the keys whose tags reference it.
    let r = row(&mut api, &mut app, "rifle");
    let rifle_tag = tag(b"weap", "objects\\weapons\\rifle", 1, Some(&pixels));
    assert_eq!(
        r["hash"],
        json!(::tags::repo::hex(&::tags::repo::normal::hash(&form(
            &rifle_tag
        ))))
    );
    assert_eq!(r["maps"], json!(["alpha", "beta"]));
    assert_eq!(r["variants"], json!([]));
    assert_eq!(
        r["used"],
        json!(["scnr:scenarios\\alpha", "scnr:scenarios\\beta"])
    );
    // Two contents: one entry per asset, with the files it is read from.
    let p = row(&mut api, &mut app, "pixels");
    assert_eq!(p["hash"], Value::Null);
    assert_eq!(p["maps"], json!(["alpha", "beta"]));
    let variants = p["variants"].as_array().unwrap().clone();
    assert_eq!(variants.len(), 2);
    let of = |maps: &str| {
        variants
            .iter()
            .find(|v| v["maps"] == json!([maps]))
            .unwrap()
    };
    assert_eq!(of("alpha")["files"], json!([null]));
    assert_eq!(of("alpha")["variant"], Value::Null);
    assert_eq!(of("beta")["files"], json!([v2.clone()]));
    assert_eq!(of("beta")["variant"], json!(v2));
    assert_eq!(of("alpha")["hash"].as_str().unwrap().len(), 64);
    assert_ne!(of("alpha")["hash"], of("beta")["hash"]);
    assert_eq!(p["used"], json!(["weap:objects\\weapons\\rifle"]));
    assert!(row(&mut api, &mut app, "scenarios\\beta")["used"].is_null());

    // A manifest that changes leaves the catalogue stale: a content is a
    // file again, and the list says so.
    let mut renamed = repo.manifest("alpha").unwrap();
    renamed.name = "renamed".into();
    repo.write_manifest(&renamed).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"query": "pixels"}),
    );
    assert_eq!(
        v["rows"][0]["variants"],
        json!([
            {"variant": null, "maps": ["alpha"]},
            {"variant": v2, "maps": ["beta"]},
        ])
    );
    assert!(v["rows"][0]["used"].is_null() && v["rows"][0]["hash"].is_null());
    let stale = v["problems"].as_array().unwrap().iter().any(|p| {
        p.as_str()
            .is_some_and(|p| p.contains("assets.idx") && p.contains("stale"))
    });
    assert!(stale, "{v}");
}

/// A catalogue that could hash none of the tree's tags (files of
/// placeholder bytes) leaves the rows as they are without one.
#[test]
fn a_catalogue_that_hashed_nothing_leaves_the_rows_as_files() {
    let t = Temp::new("assets-unhashed");
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let root = app.tags_root();
    tree(&root);
    let (c, report) =
        ::tags::repo::catalogue::build(&root, &::tags::schema::Schema::default()).unwrap();
    assert_eq!(report.problems.len(), 6, "{:?}", report.problems);
    assert!(c.assets.is_empty());
    ::tags::repo::catalogue::write(&root, &c).unwrap();
    let mut api = Api::new();
    let v = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"query": "pixels"}),
    );
    assert_eq!(
        v["rows"][0]["variants"],
        json!([
            {"variant": null, "maps": ["alpha"]},
            {"variant": "v2", "maps": ["beta"]},
        ])
    );
    assert_eq!(v["rows"][0]["maps"], json!(["alpha", "beta"]));
    assert!(v["rows"][0]["hash"].is_null() && v["rows"][0]["used"].is_null());
    let stale = v["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p.as_str().is_some_and(|p| p.contains("assets.idx")));
    assert!(!stale, "{v}");
}
