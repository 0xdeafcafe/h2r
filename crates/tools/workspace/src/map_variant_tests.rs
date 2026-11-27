//! The `map-variant` builder over an invented package and an invented
//! Forge document (no game data).
use std::collections::BTreeMap;

use forge_doc::{
    BlockKind, BuilderVersions, Edit, ForgeDoc, MapRef, Op, OpId, Placement, Sha256,
};

use crate::{Loaded, check_package, map_variants, read_map_variant};

/// A variant of the invented map `fixture` that adds one flag.
fn doc(title: &str) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(b"an invented map"),
        },
        sources: vec![],
        title: title.into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![Op {
            id: OpId(1),
            edit: Edit::Placement(Placement::Add {
                block: BlockKind::NetgameFlags,
                like: None,
                fields: vec![],
            }),
        }],
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

fn package(id: &str, items: &[(&str, Vec<u8>)]) -> Loaded {
    let mut toml = format!("[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n");
    let mut files = BTreeMap::new();
    for (source, bytes) in items {
        toml.push_str(&format!("\n[[item]]\nkind = \"map-variant\"\nsource = \"{source}\"\n"));
        files.insert((*source).to_owned(), bytes.clone());
    }
    files.insert("package.toml".to_owned(), toml.into_bytes());
    Loaded::from_files(id, &files).unwrap_or_else(|e| panic!("{e}"))
}

fn bytes(title: &str) -> Vec<u8> {
    doc(title).canonical_json().unwrap().into_bytes()
}

#[test]
fn a_map_variant_item_checks_clean_and_the_set_lists_it() {
    let castle = bytes("Castle");
    let mine = package(
        "mine",
        &[
            ("map-variants/fixture/castle.h2mapvariant", castle.clone()),
            ("map-variants/fixture/tower.h2mapvariant", bytes("Tower")),
        ],
    );
    assert_eq!(check_package(&mine), vec![]);
    let listed = map_variants(&[mine]);
    let stems: Vec<_> = listed.iter().map(|v| (v.package.as_str(), v.item, v.stem.as_str())).collect();
    assert_eq!(stems, [("mine", 0, "castle"), ("mine", 1, "tower")]);
    assert_eq!(listed[0].bytes, castle);
    assert_eq!(read_map_variant(&listed[0].bytes).unwrap().title, "Castle");
}

#[test]
fn a_source_that_is_no_variant_is_named_and_left_out_of_the_set() {
    // Not a document; a name no variant may have; a source with no plain stem.
    let mut renamed = doc("Castle");
    renamed.title = "A name far too long for a variant".into();
    let bad = package(
        "mine",
        &[
            ("map-variants/fixture/broken.h2mapvariant", b"{".to_vec()),
            ("map-variants/fixture/long.h2mapvariant", serde_json::to_vec(&renamed).unwrap()),
            ("map-variants/fixture/castle.json", bytes("Castle")),
        ],
    );
    let items: Vec<_> = check_package(&bad).into_iter().filter_map(|f| f.item).collect();
    assert_eq!(items, [0, 1, 2]);
    // The set lists what has a stem; the game's store drops what does not read.
    let stems: Vec<_> = map_variants(&[bad]).into_iter().map(|v| v.stem).collect();
    assert_eq!(stems, ["broken", "long"]);
}

#[test]
fn a_second_item_of_a_stem_is_one_variant() {
    let twice = package(
        "mine",
        &[
            ("a/castle.h2mapvariant", bytes("Castle")),
            ("b/castle.h2mapvariant", bytes("Castle Two")),
        ],
    );
    let listed = map_variants(&[twice]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, "a/castle.h2mapvariant");
}
