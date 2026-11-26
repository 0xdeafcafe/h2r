//! Tiers, play and session forms, and a variant's name
//! (docs/game/forge-map-variants.md, "The map variant file"). All inputs
//! are invented.
use forge_doc::{
    BlockKind, BuilderVersions, Edit, Endpoint, EntryOp, Expected, FieldWrite, Forge, ForgeDoc,
    ImportId, ImportSlot, KIT_RECIPE, Label, Link, MapId, MapRef, Op, OpId, Placement, Print,
    Recipe, Refusal, Sha256, TagKey, TagWrite, Tier, Value, kit, play_form, session_form,
    variant_name,
};

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn draft(edits: Vec<Edit>) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256([0x11; 32]),
        },
        sources: vec![],
        title: "Two Towers".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: edits
            .into_iter()
            .enumerate()
            .map(|(i, edit)| Op {
                id: OpId(i as u32 + 1),
                edit,
            })
            .collect(),
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

fn flag(index: u32) -> Endpoint {
    Endpoint::Base {
        block: FLAGS,
        index,
        print: Some(Print([index as u8 + 1; 8])),
    }
}

fn team() -> FieldWrite {
    FieldWrite {
        field: "team".into(),
        value: Value::Int(1),
    }
}

fn add() -> Edit {
    Edit::Placement(Placement::Add {
        block: FLAGS,
        like: None,
        fields: vec![],
    })
}

fn key() -> TagKey {
    TagKey {
        group: "weap".into(),
        path: "objects\\test\\blaster".into(),
    }
}

/// A document whose ops are all a variant may hold.
fn variant() -> ForgeDoc {
    draft(vec![
        add(),
        Edit::Entry(EntryOp::Set {
            target: flag(0),
            write: team(),
        }),
        Edit::Entry(EntryOp::Remove { target: flag(1) }),
        Edit::Forge(Forge::Label {
            target: Endpoint::Added(OpId(1)),
            label: Some(Label::new("goal", None).unwrap()),
        }),
    ])
}

#[test]
fn a_table_of_documents_and_their_tiers() {
    let with_recipes = |recipes: Vec<Recipe>| {
        let mut doc = variant();
        doc.recipes = recipes;
        doc
    };
    let cases: Vec<(&str, ForgeDoc, Tier)> = vec![
        ("an empty document", draft(vec![]), Tier::Variant),
        ("adds, entry ops and a label", variant(), Tier::Variant),
        (
            "a reset",
            draft(vec![
                Edit::Entry(EntryOp::Set {
                    target: flag(0),
                    write: team(),
                }),
                Edit::Entry(EntryOp::Reset { target: flag(0) }),
            ]),
            Tier::Variant,
        ),
        (
            "a link and an unlink",
            draft(vec![
                Edit::Forge(Forge::Link(Link::Teleporter {
                    sender: Endpoint::base(FLAGS, 0),
                    receiver: Endpoint::base(FLAGS, 1),
                    channel: 1,
                })),
                Edit::Forge(Forge::Unlink { link: OpId(1) }),
            ]),
            Tier::Variant,
        ),
        (
            "the session's kit",
            with_recipes(vec![kit()]),
            Tier::Variant,
        ),
        (
            "a placement set by running index",
            draft(vec![Edit::Placement(Placement::Set {
                block: FLAGS,
                index: 0,
                write: team(),
            })]),
            Tier::Map,
        ),
        (
            "a placement removal by running index",
            draft(vec![Edit::Placement(Placement::Remove {
                block: FLAGS,
                index: 0,
            })]),
            Tier::Map,
        ),
        (
            "a tag write",
            draft(vec![Edit::TagField(TagWrite {
                tag: key(),
                path: "rounds per shot".into(),
                value: Value::Int(2),
                was: None,
            })]),
            Tier::Map,
        ),
        (
            "a generator other than the kit",
            with_recipes(vec![Recipe::Generate {
                generator: "box".into(),
                parameters: vec![],
            }]),
            Tier::Map,
        ),
        (
            "the kit's generator with parameters",
            with_recipes(vec![Recipe::Generate {
                generator: KIT_RECIPE.into(),
                parameters: vec![FieldWrite {
                    field: "slots".into(),
                    value: Value::Int(4),
                }],
            }]),
            Tier::Map,
        ),
        (
            "a transplant beside the kit",
            with_recipes(vec![
                kit(),
                Recipe::Transplant {
                    tag: key(),
                    source: Sha256([0x11; 32]),
                },
            ]),
            Tier::Map,
        ),
        (
            "a source map",
            {
                let mut doc = variant();
                doc.sources.push(MapRef {
                    map: "donor".into(),
                    sha256: Sha256([0x22; 32]),
                });
                doc
            },
            Tier::Map,
        ),
        (
            "an import slot",
            {
                let mut doc = variant();
                doc.imports.push(ImportSlot {
                    id: ImportId(1),
                    name: "Prop".into(),
                    sha256: Sha256([0x33; 32]),
                    chassis: None,
                });
                doc
            },
            Tier::Map,
        ),
        ("a mesh op", draft(vec![Edit::Mesh]), Tier::Map),
    ];
    for (name, doc, tier) in cases {
        assert_eq!(doc.tier(), tier, "{name}");
    }
    assert_eq!(Tier::Variant.name(), "variant");
    assert_eq!(Tier::Map.name(), "map");
}

#[test]
fn the_play_form_of_the_session_form_is_the_document() {
    let mut map = variant();
    map.recipes.push(Recipe::Generate {
        generator: "box".into(),
        parameters: vec![],
    });
    for doc in [draft(vec![]), variant(), map] {
        let session = session_form(&doc);
        assert_eq!(play_form(&session), doc);
        assert!(session.recipes.contains(&kit()));
        assert_eq!(session.recipes.last(), Some(&kit()));
        assert_ne!(session.id().unwrap(), doc.id().unwrap());
        // Each form of a form is itself, and the tier is the same.
        assert_eq!(session_form(&session), session);
        assert_eq!(play_form(&doc), doc);
        assert_eq!(session.tier(), doc.tier());
    }
}

#[test]
fn a_form_that_changes_the_identity_drops_the_expectation() {
    let expect = Some(Expected {
        map_id: MapId([0x44; 32]),
        geometry: Sha256([0x55; 32]),
    });
    let mut doc = variant();
    doc.expect = expect;
    assert_eq!(session_form(&doc).expect, None);
    // A form that is already the document keeps it.
    assert_eq!(play_form(&doc).expect, expect);
    let mut session = session_form(&variant());
    session.expect = expect;
    assert_eq!(session_form(&session).expect, expect);
    assert_eq!(play_form(&session).expect, None);
}

#[test]
fn a_variant_name_is_one_to_sixteen_characters() {
    for good in ["A", "Two Towers", "abcdefghijklmnop", "éééééééééééééééé"] {
        variant_name(good).unwrap();
    }
    for (bad, why) in [
        ("abcdefghijklmnopq", "at most 16"),
        ("", "must have a name"),
        ("   ", "must have a name"),
        ("Two\nTowers", "control"),
    ] {
        let error = variant_name(bad).unwrap_err();
        assert!(error.to_string().contains(why), "{bad:?}: {error}");
    }

    let mut doc = variant();
    doc.check_variant().unwrap();
    doc.title = "x".repeat(17);
    assert!(doc.check_variant().is_err());
    // A Forge map may keep a longer title: the rule is a variant's.
    let mut map = draft(vec![Edit::Placement(Placement::Remove {
        block: FLAGS,
        index: 0,
    })]);
    map.title = "x".repeat(17);
    forge_doc::validate(&map, &forge_doc::Limits::default()).unwrap();
    assert_eq!(map.check_variant(), Err(Refusal::Tier));
}
