//! `forge.*`, the Forge variants and Forge items kinds of `asset.list`,
//! and a variant's links (docs/editor/studio-flow.md, "Forge variants and
//! items"), over an invented map variant saved through the game's own
//! store (`forge::files::variants`) and the invented tree and package of
//! `assets_tests`. No game data.

use super::assets_tests;
use super::*;
use crate::app::{Action, Doc};
use forge_doc::{
    BlockKind, BuilderVersions, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, MapRef, Op,
    OpId, Placement, Sha256, TagKey as ForgeTag, Value as Field,
};

const RIFLE: &str = "tag:weap:objects\\weapons\\rifle";

fn vec3(v: [f32; 3]) -> Field {
    Field::Vec3(v.map(|f| F32::new(f).unwrap()))
}

/// `Castle`, a variant of the invented map `fixture`: two weapons added,
/// the tree's rifle and one more, the second moved by a later entry write.
pub(super) fn castle() -> ForgeDoc {
    let add = |id: u32, path: &str, at: [f32; 3]| Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block: BlockKind::Weapons,
            like: None,
            fields: vec![
                FieldWrite {
                    field: "palette index".into(),
                    value: Field::Tag(Some(ForgeTag {
                        group: "weap".into(),
                        path: path.into(),
                    })),
                },
                FieldWrite { field: "position".into(), value: vec3(at) },
            ],
        }),
    };
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(b"an invented map"),
        },
        sources: vec![],
        title: "Castle".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![
            add(1, "objects\\weapons\\rifle", [1.0, 2.0, 3.0]),
            add(2, "objects\\weapons\\invented", [4.0, 5.0, 6.0]),
            Op {
                id: OpId(3),
                edit: Edit::Entry(EntryOp::Set {
                    target: Endpoint::Added(OpId(2)),
                    write: FieldWrite { field: "position".into(), value: vec3([7.0, 8.0, 9.0]) },
                }),
            },
        ],
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

/// Save `Castle` and open it as a document; its asset id.
pub(super) fn open_castle(api: &mut Api, app: &mut App) -> String {
    forge::files::variants::save(&app.data_root(), &castle(), false).unwrap();
    let listed = call(api, app, m::FORGE_VARIANT_LIST, json!({}));
    let asset = listed["rows"][0]["asset"].as_str().unwrap().to_owned();
    call(api, app, m::SHELL_OPEN, json!({ "asset": asset }));
    asset
}

fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    (t, app, Api::new())
}

fn field<'a>(v: &'a Value, target: &str, name: &str) -> &'a Value {
    let thing = v["things"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"] == target)
        .unwrap_or_else(|| panic!("no {target} in {v}"));
    thing["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field"] == name)
        .map(|f| &f["value"])
        .unwrap_or_else(|| panic!("no {name} of {target} in {v}"))
}

#[test]
fn the_forge_variants_kind_lists_a_saved_variant_and_it_opens_as_a_document() {
    let (_t, mut app, mut api) = setup("forge-open");
    let none = call(&mut api, &mut app, m::FORGE_VARIANT_LIST, json!({}));
    assert_eq!(none["rows"], json!([]));
    let asset = open_castle(&mut api, &mut app);
    assert!(asset.starts_with("forge:fixture:"), "{asset}");
    assert_eq!(app.front_doc(), Some(Doc::Forge));
    assert_eq!(app.doc_id(Doc::Forge), asset);
    assert_eq!(app.doc_title(Doc::Forge), "Castle");
    assert_eq!(app.doc_asset(Doc::Forge).as_deref(), Some(asset.as_str()));
    // The Browser's kind answers the same row.
    let kind = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({ "kind": "Forge variants", "query": "castle" }),
    );
    assert_eq!(kind["rows"][0]["asset"], asset.as_str());
    assert_eq!(kind["rows"][0]["things"], 2);
    assert_eq!(kind["rows"][0]["source"], "saved");
    assert!(kind["kinds"].as_array().unwrap().iter().any(|k| k == "Forge items"));
    // The document: the two added weapons, the second where its last
    // write put it.
    let doc = call(&mut api, &mut app, m::FORGE_VARIANT_READ, json!({}));
    assert_eq!(doc["name"], "Castle");
    assert_eq!(doc["open"], true);
    assert_eq!(doc["dirty"], false);
    assert_eq!(doc["things"].as_array().unwrap().len(), 2);
    assert_eq!(doc["things"][0]["item"], RIFLE);
    assert_eq!(field(&doc, "added:2", "position"), &json!([7.0, 8.0, 9.0]));
    assert_eq!(field(&doc, "added:1", "palette index"), &json!("weap:objects\\weapons\\rifle"));
}

#[test]
fn a_field_edit_is_one_undo_step_and_save_writes_the_stores_file() {
    let (_t, mut app, mut api) = setup("forge-set");
    let asset = open_castle(&mut api, &mut app);
    let (map, file) = asset
        .strip_prefix("forge:")
        .and_then(|rest| rest.split_once(':'))
        .unwrap();
    let set = |api: &mut Api, app: &mut App, value: Value| {
        call(
            api,
            app,
            m::FORGE_VARIANT_SET,
            json!({ "target": "added:2", "field": "position", "value": value }),
        )
    };
    let v = set(&mut api, &mut app, json!([0, 0, 1]));
    assert_eq!(field(&v, "added:2", "position"), &json!([0.0, 0.0, 1.0]));
    assert_eq!(v["dirty"], true);
    assert!(app.doc_dirty(Doc::Forge));
    assert_eq!(app.doc_can(Doc::Forge), (true, false));
    // The header's Undo and Redo are the document's.
    app.dispatch(Action::UndoDoc(Doc::Forge));
    assert!(!app.doc_dirty(Doc::Forge));
    assert_eq!(app.doc_can(Doc::Forge), (false, true));
    app.dispatch(Action::RedoDoc(Doc::Forge));
    assert!(app.doc_dirty(Doc::Forge));
    // The file keeps the old value until the save.
    let on_disk = |app: &App| {
        let doc = app.forge_variant_doc(map, file).unwrap();
        crate::app::forge_variants::doc_json(map, file, &doc)
    };
    assert_eq!(field(&on_disk(&app), "added:2", "position"), &json!([7.0, 8.0, 9.0]));
    assert!(app.doc_save_action(Doc::Forge).is_some());
    let saved = call(&mut api, &mut app, m::FORGE_VARIANT_SAVE, json!({}));
    assert_eq!(saved["dirty"], false);
    assert_eq!(saved["asset"], asset.as_str());
    assert_eq!(field(&on_disk(&app), "added:2", "position"), &json!([0.0, 0.0, 1.0]));
    assert!(app.doc_save_action(Doc::Forge).is_none(), "nothing left to save");
}

#[test]
fn a_value_of_another_type_a_field_not_written_and_no_open_variant_are_refused() {
    let (_t, mut app, mut api) = setup("forge-refused");
    let e = api
        .call(
            &mut app,
            m::FORGE_VARIANT_SET,
            json!({ "target": "added:1", "field": "position", "value": [1, 2, 3] }),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    open_castle(&mut api, &mut app);
    for (target, name, value, code) in [
        ("added:1", "position", json!("far"), code::INVALID_PARAMS),
        ("added:1", "position", json!([1, 2]), code::INVALID_PARAMS),
        ("added:1", "scale", json!(2), code::REFUSED),
        ("added:9", "position", json!([1, 2, 3]), code::REFUSED),
        ("nonsense", "position", json!([1, 2, 3]), code::INVALID_PARAMS),
    ] {
        let e = api
            .call(
                &mut app,
                m::FORGE_VARIANT_SET,
                json!({ "target": target, "field": name, "value": value }),
            )
            .unwrap_err();
        assert_eq!(e.code, code, "{target} {name}: {}", e.message);
    }
    assert!(!app.doc_dirty(Doc::Forge), "a refused edit changes nothing");
    // A variant with unsaved edits stays open.
    call(
        &mut api,
        &mut app,
        m::FORGE_VARIANT_SET,
        json!({ "target": "added:1", "field": "position", "value": [1, 2, 4] }),
    );
    app.dispatch(Action::CloseDoc(Doc::Forge));
    assert!(app.forge_variants.open.is_some());
    app.dispatch(Action::UndoDoc(Doc::Forge));
    app.dispatch(Action::CloseDoc(Doc::Forge));
    assert!(app.forge_variants.open.is_none());
}

#[test]
fn an_item_lists_the_variants_that_place_it_and_a_variant_the_items_it_places() {
    let (_t, mut app, mut api) = setup("forge-links");
    let asset = open_castle(&mut api, &mut app);
    let used = call(&mut api, &mut app, m::ASSET_WHERE_USED, json!({ "asset": RIFLE }));
    let rows = used["rows"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["role"] == "Forge variant")
        .unwrap_or_else(|| panic!("{used}"));
    assert_eq!(row["asset"], asset.as_str());
    assert_eq!(row["via"], "1 placed in Castle");
    // A tag no variant places has no such row.
    let none = call(
        &mut api,
        &mut app,
        m::ASSET_WHERE_USED,
        json!({ "asset": "tag:weap:objects\\weapons\\nothing" }),
    );
    assert!(none["rows"].as_array().unwrap().iter().all(|r| r["role"] != "Forge variant"));
    // The variant names what it places, and its own editor.
    let links = call(&mut api, &mut app, m::ASSET_LINKS, json!({ "asset": asset }));
    let uses: Vec<&str> = links["uses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["asset"].as_str().unwrap())
        .collect();
    assert_eq!(uses, ["tag:weap:objects\\weapons\\invented", RIFLE]);
    assert_eq!(links["uses"][0]["via"], "1 placed");
    assert_eq!(links["editors"][0]["id"], "forge_variant");
}

#[test]
fn forge_items_lists_the_disc_items_the_package_items_and_the_block_out_set() {
    let (t, mut app, mut api) = setup("forge-items");
    assets_tests::tree(&app.tags_root());
    assets_tests::package(&t.path("data/packages/kit-test"));
    let all = call(&mut api, &mut app, m::FORGE_ITEMS, json!({ "limit": 1000 }));
    let rows = all["rows"].as_array().unwrap();
    let of = |path: &str| {
        rows.iter()
            .find(|r| r["path"] == path)
            .unwrap_or_else(|| panic!("no {path} in {all}"))
    };
    // The tree's weapon is a disc item, the package's a custom one.
    assert_eq!(of("objects\\weapons\\rifle")["forge"], json!({"menu": "Weapons", "custom": false, "source": "disc"}));
    assert_eq!(of("objects\\weapons\\kit_new")["forge"], json!({"menu": "Weapons", "custom": true, "source": "package"}));
    // A bitmap and a scenario are not items.
    assert!(rows.iter().all(|r| r["group"] != "bitm" && r["group"] != "scnr"), "{all}");
    let blockout = forge_shapes::starter().len();
    assert_eq!(all["custom"], blockout + 1);
    // `custom` keeps the custom ones; the Browser's kind answers the same rows.
    let custom = call(&mut api, &mut app, m::FORGE_ITEMS, json!({ "custom": true, "limit": 1000 }));
    assert_eq!(custom["total"], blockout + 1);
    assert!(custom["rows"].as_array().unwrap().iter().all(|r| r["forge"]["custom"] == true));
    let kind = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({ "kind": "Forge items", "query": "kit_new" }),
    );
    assert_eq!(kind["total"], 1);
    assert_eq!(kind["rows"][0]["asset"], "tag:weap:objects\\weapons\\kit_new");
    assert_eq!(kind["rows"][0]["kind"], "Forge items");
}
