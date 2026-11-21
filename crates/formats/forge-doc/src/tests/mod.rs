use crate::*;

mod bounds;
mod log;

const EMPTY: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":1,"imports":[],"ops":[],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;
const ONE_OP: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":1,"imports":[],"ops":[{"edit":{"placement":{"set":{"block":"weapons","index":0,"write":{"field":"scale","value":{"float":"80000000"}}}}},"id":1}],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;

fn draft() -> ForgeDoc {
    ForgeDoc {
        format: FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 2,
            bots: 3,
        },
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256([0x11; 32]),
        },
        sources: vec![],
        title: "Forge \"é\"".into(),
        author: "Editor".into(),
        description: "row 1\nrow 2".into(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    }
}

fn key(group: &str) -> TagKey {
    TagKey {
        group: group.into(),
        path: "objects\\test\\example".into(),
    }
}

fn set(id: u32, block: BlockKind, index: u32, field: &str, value: Value) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Set {
            block,
            index,
            write: FieldWrite {
                field: field.into(),
                value,
            },
        }),
    }
}

fn scale() -> Op {
    set(
        1,
        BlockKind::Weapons,
        0,
        "scale",
        Value::Float(F32::new(-0.0).unwrap()),
    )
}

fn add(id: u32, block: BlockKind) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block,
            like: None,
            fields: vec![],
        }),
    }
}

fn tag_write() -> TagWrite {
    TagWrite {
        tag: key("shad"),
        path: "postprocess[0].colors[1].color".into(),
        value: Value::Vec3([F32::new(0.25).unwrap(); 3]),
        was: Some(Value::Vec3([F32::new(1.0).unwrap(); 3]).digest().unwrap()),
    }
}

#[test]
fn canonical_document_golden_vectors() {
    let limits = Limits::default();
    let mut doc = draft();
    assert_eq!(doc.canonical_json().unwrap(), EMPTY);
    assert_eq!(
        doc.id().unwrap().to_string(),
        "77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001"
    );
    doc.ops.push(scale());
    assert_eq!(doc.canonical_json().unwrap(), ONE_OP);
    assert_eq!(
        doc.id().unwrap().to_string(),
        "7a64929ecc49d0a2f743128ee53371f8d3aed410e7f70fbfd1b2858551cdba6d"
    );
    assert_eq!(
        ForgeDoc::from_bytes(ONE_OP.as_bytes(), &limits).unwrap(),
        doc
    );

    let reordered = serde_json::to_string_pretty(&doc).unwrap();
    let reordered = reordered.replace("\"title\":", "\"title\" : ");
    let reread = ForgeDoc::from_bytes(reordered.as_bytes(), &limits).unwrap();
    assert_eq!(reread.id().unwrap(), doc.id().unwrap());
    assert_ne!(
        Value::Float(F32::new(0.0).unwrap()).digest().unwrap(),
        Value::Float(F32::new(-0.0).unwrap()).digest().unwrap()
    );
}

#[test]
fn portable_references_and_recipes_round_trip_in_order() {
    let mut doc = draft();
    let donor = MapRef {
        map: "donor".into(),
        sha256: Sha256([0x22; 32]),
    };
    let slot = ImportSlot {
        id: ImportId(7),
        sha256: Sha256([0x33; 32]),
        name: "Example skin".into(),
        chassis: Some(key("bipd")),
    };
    let write = tag_write();
    doc.imports.push(slot);
    doc.sources.push(donor.clone());
    doc.recipes = vec![
        Recipe::Transplant {
            tag: key("weap"),
            source: donor.sha256,
        },
        Recipe::Variant {
            tag: write.tag.clone(),
            writes: vec![write.clone()],
        },
        Recipe::Generate {
            generator: "box".into(),
            parameters: vec![
                FieldWrite {
                    field: "solid".into(),
                    value: Value::Bool(true),
                },
                FieldWrite {
                    field: "name".into(),
                    value: Value::Text("Example box".into()),
                },
                FieldWrite {
                    field: "sides".into(),
                    value: Value::Int(6),
                },
            ],
        },
        Recipe::Import { slot: ImportId(7) },
    ];
    doc.ops = vec![
        set(
            2,
            BlockKind::Weapons,
            0,
            "palette index",
            Value::Tag(Some(key("weap"))),
        ),
        Op {
            id: OpId(3),
            edit: Edit::TagField(write),
        },
    ];
    doc.expect = Some(Expected {
        map_id: MapId([0x44; 32]),
        geometry: Sha256([0x55; 32]),
    });

    let json = doc.canonical_json().unwrap();
    assert_eq!(
        ForgeDoc::from_bytes(json.as_bytes(), &Limits::default()).unwrap(),
        doc
    );
    let h = doc.id().unwrap();
    doc.ops.swap(0, 1);
    assert_ne!(doc.id().unwrap(), h);
    doc.ops.swap(0, 1);
    doc.recipes.swap(0, 1);
    assert_ne!(doc.id().unwrap(), h);
}

#[test]
fn expectations_do_not_change_document_or_log_identity() {
    let mut doc = draft();
    doc.ops.push(scale());
    let id = doc.id().unwrap();
    let genesis = LogHead::genesis(&doc).unwrap();
    let original_file = doc.canonical_json().unwrap();

    for byte in [1, 2] {
        doc.expect = Some(Expected {
            map_id: MapId([byte; 32]),
            geometry: Sha256([byte; 32]),
        });
        assert_ne!(doc.canonical_json().unwrap(), original_file);
        assert_eq!(doc.id().unwrap(), id);
        assert_eq!(LogHead::genesis(&doc).unwrap(), genesis);
        let (after, head) = fold_from(&doc, genesis, &[], &Limits::default()).unwrap();
        assert_eq!(after, doc);
        assert_eq!(head, genesis);
    }
}
