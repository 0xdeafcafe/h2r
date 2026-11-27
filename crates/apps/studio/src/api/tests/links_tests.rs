//! `asset.links` over an invented tags tree and two local packages
//! (docs/editor/studio-flow.md, "Links"): a weapon that names a model, a
//! sound and a projectile, a dialogue sound under an invented character's
//! folder, a tag file that does not decode, the assets_tests package
//! `kit-test` (which edits the weapon) and a package `modes-test` whose
//! `rules` and `variant` items share a script key. No game data. The
//! package id is not the built-in `modes`: a local package that takes a
//! baked package's id stays out whole.

use super::assets_tests;
use super::*;
use ::tags::repo::store::Manifest;
use ::tags::repo::{Body, LooseTag, Node, Region, TagKey, TagRepo, Tree, Value as Field};
use map::FourCC;

fn key(g: &[u8; 4], path: &str) -> TagKey {
    TagKey::new(FourCC(*g), path)
}

/// A tag whose one array holds a reference at each of `refs`' offsets.
fn tag(k: &TagKey, refs: &[(u32, &TagKey)]) -> LooseTag {
    LooseTag {
        key: k.clone(),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![0; 32],
                count: 1,
                size: 32,
                align: 4,
                fields: refs
                    .iter()
                    .map(|(at, r)| (*at, Field::Ref((*r).clone())))
                    .collect(),
            }],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

const RIFLE: &str = "tag:weap:objects\\weapons\\rifle";
const MODEL: &str = "tag:hlmt:objects\\weapons\\rifle_model";
const FIRE: &str = "tag:snd!:sound\\weapons\\rifle_fire";
const LINE: &str = "tag:snd!:sound\\dialog\\arbiter\\l01_arb";

fn tree(root: &std::path::Path) {
    let mut repo = TagRepo::create(root).unwrap();
    let rifle = key(b"weap", "objects\\weapons\\rifle");
    let model = key(b"hlmt", "objects\\weapons\\rifle_model");
    let fire = key(b"snd!", "sound\\weapons\\rifle_fire");
    let bullet = key(b"proj", "objects\\weapons\\bullet");
    let line = key(b"snd!", "sound\\dialog\\arbiter\\l01_arb");
    let tags = [
        tag(&rifle, &[(0, &model), (8, &fire), (16, &bullet)]),
        tag(&model, &[]),
        tag(&fire, &[]),
        tag(&bullet, &[]),
        tag(&line, &[]),
    ];
    let mut keys = Vec::new();
    for t in &tags {
        repo.put(t).unwrap();
        keys.push((t.key.clone(), None));
    }
    // A file that holds no tag: the scan names it and goes on.
    let broken = key(b"bitm", "objects\\broken");
    let p = repo.path_of(&broken, None);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"placeholder").unwrap();
    keys.push((broken, None));
    repo.put_manifest(
        &Manifest {
            map: "alpha".into(),
            name: "alpha".into(),
            tags: keys,
            ..Default::default()
        },
        b"header",
    )
    .unwrap();
}

/// `modes-test`: a script `hill` as a `rules` item and as a `variant`
/// item, another script, and a requirement on `kit-test`.
fn modes(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("package.toml"),
        r#"[package]
format = 1
id = "modes-test"
name = "Modes test"
requires = ["kit-test"]

[[item]]
kind = "rules"
source = "scripts/hill.rules"

[[item]]
kind = "variant"
source = "scripts/hill.variant.rules"

[[item]]
kind = "rules"
source = "scripts/other.rules"
"#,
    )
    .unwrap();
    for f in ["hill.rules", "hill.variant.rules", "other.rules"] {
        std::fs::write(dir.join("scripts").join(f), "rules\n").unwrap();
    }
}

fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    tree(&app.tags_root());
    assets_tests::package(&t.path("data/packages/kit-test"));
    modes(&t.path("data/packages/modes-test"));
    (t, app, Api::new())
}

fn links(api: &mut Api, app: &mut App, asset: &str) -> Value {
    call(api, app, m::ASSET_LINKS, json!({ "asset": asset }))
}

/// `(role, asset)` of each link of a list.
fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["role"].as_str().unwrap().to_owned(),
                l["asset"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn has(v: &Value, role: &str, asset: &str) -> bool {
    pairs(v).contains(&(role.to_owned(), asset.to_owned()))
}

#[test]
fn a_weapon_lists_its_model_sound_and_projectile_by_role() {
    let (_t, mut app, mut api) = setup("links-weapon");
    let v = links(&mut api, &mut app, RIFLE);
    assert_eq!(
        pairs(&v["uses"]),
        [
            ("Model".to_owned(), MODEL.to_owned()),
            ("Sounds".to_owned(), FIRE.to_owned()),
            ("Projectile".to_owned(), "tag:proj:objects\\weapons\\bullet".to_owned()),
        ],
        "{v}"
    );
    assert_eq!(v["uses"][0]["via"], "node 0 +0x0");
    assert_eq!(v["uses"][1]["via"], "node 0 +0x8");
    // The weapon is a tag of the Objects kind: Fields and the editors of
    // other kinds that work on it.
    let editors: Vec<&str> = v["editors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(editors, ["field_panel", "modeling", "hud"], "{v}");
}

#[test]
fn a_model_lists_the_weapon_under_used_by() {
    let (_t, mut app, mut api) = setup("links-model");
    let v = links(&mut api, &mut app, MODEL);
    assert!(has(&v["used_by"], "Objects", RIFLE), "{v}");
    assert_eq!(v["used_by"][0]["via"], "node 0 +0x0");
    assert_eq!(v["uses"], json!([]));
}

#[test]
fn a_tag_an_edit_line_touches_lists_the_package_and_a_package_tag_its_adder() {
    let (_t, mut app, mut api) = setup("links-package");
    let v = links(&mut api, &mut app, RIFLE);
    let by: Vec<_> = v["used_by"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["role"] == "Package")
        .collect();
    assert_eq!(by.len(), 1, "{v}");
    assert_eq!(by[0]["asset"], "package:kit-test");
    assert_eq!(by[0]["via"], "edits");

    // A tag a package adds has no file in the tree.
    let v = links(&mut api, &mut app, "tag:weap:objects\\weapons\\kit_new");
    assert!(has(&v["used_by"], "Package", "package:kit-test"), "{v}");
    assert_eq!(v["used_by"][0]["via"], "adds");
}

#[test]
fn a_variant_lists_its_script_and_the_script_its_variant() {
    let (_t, mut app, mut api) = setup("links-script");
    let variant = links(&mut api, &mut app, "item:modes-test:1");
    assert_eq!(
        pairs(&variant["uses"]),
        [("Script".to_owned(), "item:modes-test:0".to_owned())],
        "{variant}"
    );
    assert!(has(&variant["used_by"], "Package", "package:modes-test"));
    let script = links(&mut api, &mut app, "item:modes-test:0");
    assert_eq!(
        pairs(&script["used_by"]).into_iter().filter(|p| p.0 == "Variant").collect::<Vec<_>>(),
        [("Variant".to_owned(), "item:modes-test:1".to_owned())],
        "{script}"
    );
    // Another script shares no key: nothing links it to the variant.
    let other = links(&mut api, &mut app, "item:modes-test:2");
    assert!(!has(&other["used_by"], "Variant", "item:modes-test:1"), "{other}");
}

#[test]
fn a_dialogue_sound_lists_the_voice_that_says_it() {
    let (_t, mut app, mut api) = setup("links-voice");
    let v = links(&mut api, &mut app, LINE);
    assert_eq!(
        pairs(&v["uses"]),
        [("Voice".to_owned(), "voice:arbiter".to_owned())],
        "{v}"
    );
    assert_eq!(v["uses"][0]["via"], "dialogue rules");
    let editors: Vec<&str> = v["editors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(editors.contains(&"voices"), "{v}");
    // A weapon's sound is no dialogue.
    let v = links(&mut api, &mut app, FIRE);
    assert!(pairs(&v["uses"]).is_empty(), "{v}");
    assert!(has(&v["used_by"], "Objects", RIFLE));
}

#[test]
fn packages_and_items_link_to_each_other() {
    let (_t, mut app, mut api) = setup("links-items");
    let v = links(&mut api, &mut app, "package:kit-test");
    assert!(has(&v["uses"], "Package", "item:kit-test:1"), "{v}");
    assert!(has(&v["used_by"], "Package", "package:modes-test"), "{v}");
    let v = links(&mut api, &mut app, "package:modes-test");
    assert!(has(&v["uses"], "Package", "package:kit-test"), "{v}");

    // The edits item uses the tag its line names; a tag item the tag it adds.
    let v = links(&mut api, &mut app, "item:kit-test:1");
    assert!(has(&v["uses"], "Objects", RIFLE), "{v}");
    assert!(has(&v["used_by"], "Package", "package:kit-test"), "{v}");
    let v = links(&mut api, &mut app, "item:kit-test:0");
    assert!(has(&v["uses"], "Objects", "tag:weap:objects\\weapons\\kit_new"), "{v}");
}

#[test]
fn a_file_that_does_not_decode_is_named_and_the_rest_is_answered() {
    let (_t, mut app, mut api) = setup("links-problems");
    let v = links(&mut api, &mut app, MODEL);
    let problems = v["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{v}");
    assert!(
        problems[0].as_str().unwrap().starts_with("tag:bitm:objects\\broken"),
        "{v}"
    );
    assert!(has(&v["used_by"], "Objects", RIFLE));
}

#[test]
fn a_bad_id_a_missing_package_and_an_item_past_the_end_are_refused() {
    let (_t, mut app, mut api) = setup("links-refused");
    for (asset, code) in [
        ("nonsense", code::INVALID_PARAMS),
        ("package:nope", code::REFUSED),
        ("item:kit-test:99", code::REFUSED),
        ("item:nope:0", code::REFUSED),
    ] {
        let e = api
            .call(&mut app, m::ASSET_LINKS, json!({ "asset": asset }))
            .unwrap_err();
        assert_eq!(e.code, code, "{asset}: {}", e.message);
    }
}

#[test]
fn a_script_document_is_the_item_of_the_package_that_carries_it() {
    use crate::app::{Doc, assets::read_set, links::item_id};
    use content::package::Kind;
    let (t, mut app, _api) = setup("links-doc-asset");
    let script = |f: &str| Some(t.path(&format!("data/packages/modes-test/scripts/{f}")));
    app.rules.path = script("hill.rules");
    assert_eq!(app.doc_asset(Doc::Rules).as_deref(), Some("item:modes-test:0"));
    app.rules.path = script("other.rules");
    assert_eq!(app.doc_asset(Doc::Rules).as_deref(), Some("item:modes-test:2"));
    app.rules.path = script("nowhere.rules");
    assert_eq!(app.doc_asset(Doc::Rules), None);
    // A variant and a playlist file are named the same way; none open is none.
    let set = read_set(&app.data_root());
    assert_eq!(item_id(&set, Kind::Variant, "hill").as_deref(), Some("item:modes-test:1"));
    assert_eq!(item_id(&set, Kind::Variant, "HILL").as_deref(), Some("item:modes-test:1"));
    assert_eq!(item_id(&set, Kind::Playlist, "hill"), None);
    assert_eq!(app.doc_asset(Doc::Variant), None);
    assert_eq!(app.doc_asset(Doc::Playlists), None);
    // The header's path for an item names its package, its kind and its source.
    let c = crate::ui::doc_header_crumbs(&app, "item:modes-test:1").unwrap();
    assert_eq!(
        (c.origin.as_str(), c.kind.as_str(), c.leaf.as_str()),
        ("modes-test", "variant", "scripts/hill.variant.rules")
    );
}

#[test]
fn a_voice_lists_the_dialogue_sounds_its_rules_pick() {
    let (_t, mut app, mut api) = setup("links-voice");
    let v = links(&mut api, &mut app, "voice:arbiter");
    assert_eq!(
        pairs(&v["used_by"]),
        [("Sounds".to_owned(), LINE.to_owned())],
        "{v}"
    );
    assert_eq!(v["editors"][0]["id"], "voices");
    // The sound names its voice, and the voice opens as an asset id.
    let sound = links(&mut api, &mut app, LINE);
    assert!(has(&sound["uses"], "Voice", "voice:arbiter"), "{sound}");
    // A voice that is no character is refused as a bad id.
    let e = api
        .call(&mut app, m::ASSET_LINKS, json!({ "asset": "voice:nobody" }))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "{}", e.message);
}

/// **Where used** (`asset.where_used`, `Action::WhereUsed`): the rows are
/// `used_by` of the link table, and the Browser holds them as its view.
#[test]
fn where_used_answers_used_by_and_the_browser_shows_the_same_rows() {
    use crate::app::Action;
    use crate::app::assets::BrowseMode;
    let (_t, mut app, mut api) = setup("links-where-used");
    let links = links(&mut api, &mut app, MODEL);
    let v = call(&mut api, &mut app, m::ASSET_WHERE_USED, json!({ "asset": MODEL }));
    assert_eq!(v["view"], "where_used");
    assert_eq!(v["asset"], MODEL);
    assert_eq!(v["rows"], links["used_by"], "{v}");
    assert_eq!(v["total"], 1);
    let view = app.browse.view.clone().expect("the Browser's view");
    assert_eq!(view.mode, BrowseMode::WhereUsed);
    assert_eq!(view.rows[0].asset, RIFLE);
    assert_eq!(view.heading(), "Where used: rifle_model (1)");
    // The window's entry leaves the same view, and Clear takes it away.
    let serial = app.browse.serial;
    app.dispatch(Action::WhereUsed { asset: MODEL.to_owned() });
    assert_eq!(app.browse.view.as_ref(), Some(&view));
    assert_ne!(app.browse.serial, serial, "a new view comes forward");
    app.dispatch(Action::ClearBrowse);
    assert!(app.browse.view.is_none());
}

/// **Show edits** (`asset.edits`, `Action::ShowEdits`): a tag lists the
/// items with a patch on it, a package its edit items.
#[test]
fn show_edits_lists_the_items_that_change_a_tag_and_a_packages_edit_items() {
    let (_t, mut app, mut api) = setup("links-edits");
    let v = call(&mut api, &mut app, m::ASSET_EDITS, json!({ "asset": RIFLE }));
    assert_eq!(v["view"], "edits");
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["asset"], "item:kit-test:1");
    assert_eq!(rows[0]["role"], "edit");
    assert!(rows[0]["via"].as_str().unwrap().contains("x = 1"), "{v}");
    // A tag no package touches has no row; the view still shows.
    let none = call(&mut api, &mut app, m::ASSET_EDITS, json!({ "asset": MODEL }));
    assert_eq!(none["total"], 0);
    assert_eq!(app.browse.view.as_ref().map(|v| v.of.as_str()), Some(MODEL));
    // A package: its `edits` item, by its file.
    let package = call(
        &mut api,
        &mut app,
        m::ASSET_EDITS,
        json!({ "asset": "package:kit-test" }),
    );
    assert_eq!(
        package["rows"],
        json!([{ "asset": "item:kit-test:1", "role": "edit", "via": "edits/kit.yaml" }])
    );
}

#[test]
fn a_link_view_of_a_bad_id_or_an_item_without_edits_is_refused_and_keeps_the_view() {
    let (_t, mut app, mut api) = setup("links-view-refused");
    call(&mut api, &mut app, m::ASSET_WHERE_USED, json!({ "asset": MODEL }));
    for (method, asset, code) in [
        (m::ASSET_WHERE_USED, "nonsense", code::INVALID_PARAMS),
        (m::ASSET_WHERE_USED, "package:nope", code::REFUSED),
        (m::ASSET_EDITS, "item:kit-test:1", code::REFUSED),
        (m::ASSET_EDITS, "voice:arbiter", code::REFUSED),
    ] {
        let e = api
            .call(&mut app, method, json!({ "asset": asset }))
            .unwrap_err();
        assert_eq!(e.code, code, "{method} {asset}: {}", e.message);
    }
    assert_eq!(app.browse.view.as_ref().map(|v| v.of.as_str()), Some(MODEL));
}
