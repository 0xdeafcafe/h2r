use super::*;
use crate::document::{MAX_FIELDS, MAX_IMPORTS, MAX_OPS, MAX_RECIPES, MAX_SOURCES};

fn refuse(doc: &ForgeDoc, limits: &Limits, expected: &str) {
    let error = validate(doc, limits).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "{error} did not mention {expected}"
    );
}

#[test]
fn document_header_and_references_refuse_invalid_values() {
    let limits = Limits::default();
    let mut doc = draft();
    doc.format = FORMAT + 1;
    assert_eq!(validate(&doc, &limits), Err(Refusal::Format(FORMAT + 1)));
    doc.format = FORMAT;
    refuse(
        &doc,
        &Limits {
            builder: Some(BuilderVersions {
                bots: 6,
                ..doc.builder
            }),
            ..limits.clone()
        },
        "builder versions",
    );

    let mutations: [(fn(&mut ForgeDoc), &str); 15] = [
        (|doc| doc.base.map = "../fixture".into(), "map name"),
        (
            |doc| {
                doc.sources.push(MapRef {
                    map: "FIXTURE".into(),
                    sha256: Sha256([5; 32]),
                })
            },
            "listed twice",
        ),
        (
            |doc| {
                doc.sources.push(MapRef {
                    map: "another".into(),
                    sha256: doc.base.sha256,
                })
            },
            "listed twice",
        ),
        (|doc| doc.title = " ".into(), "title"),
        (|doc| doc.author = "\n".into(), "author"),
        (|doc| doc.description.push('\0'), "control character"),
        (|doc| doc.description = "a".repeat(4097), "description"),
        (
            |doc| doc.recipes.push(Recipe::Import { slot: ImportId(7) }),
            "no such import slot",
        ),
        (
            |doc| {
                doc.recipes.push(Recipe::Transplant {
                    tag: key("weap"),
                    source: Sha256([5; 32]),
                })
            },
            "transplant source",
        ),
        (
            |doc| {
                doc.recipes.push(Recipe::Generate {
                    generator: "../box".into(),
                    parameters: vec![],
                })
            },
            "generator",
        ),
        (
            |doc| {
                doc.recipes.push(Recipe::Generate {
                    generator: "box".into(),
                    parameters: vec![
                        FieldWrite {
                            field: "length".into(),
                            value: Value::Int(1)
                        };
                        2
                    ],
                })
            },
            "listed twice",
        ),
        (
            |doc| {
                doc.recipes.push(Recipe::Generate {
                    generator: "box".into(),
                    parameters: vec![FieldWrite {
                        field: "bad[name]".into(),
                        value: Value::Int(1),
                    }],
                })
            },
            "parameter name",
        ),
        (
            |doc| {
                doc.recipes.push(Recipe::Variant {
                    tag: key("weap"),
                    writes: vec![tag_write()],
                })
            },
            "another tag",
        ),
        (
            |doc| {
                doc.ops.push(Op {
                    id: OpId(0),
                    ..scale()
                })
            },
            "operation id",
        ),
        (
            |doc| doc.ops = vec![scale(), scale()],
            "already in the document",
        ),
    ];
    for (mutate, expected) in mutations {
        let mut doc = draft();
        mutate(&mut doc);
        refuse(&doc, &limits, expected);
    }

    let slot = ImportSlot {
        id: ImportId(1),
        name: "Skin".into(),
        sha256: Sha256([3; 32]),
        chassis: None,
    };
    let mut doc = draft();
    doc.imports.push(ImportSlot {
        id: ImportId(0),
        ..slot.clone()
    });
    refuse(&doc, &limits, "import slot id");
    doc.imports = vec![slot.clone(); 2];
    refuse(&doc, &limits, "import slot id");
    doc.imports = vec![ImportSlot {
        name: "".into(),
        ..slot.clone()
    }];
    refuse(&doc, &limits, "import name");
    doc.imports = vec![ImportSlot {
        chassis: Some(key("weap")),
        ..slot
    }];
    refuse(&doc, &limits, "import chassis");

    for edit in [Edit::Mesh, Edit::Bitmap, Edit::Forge, Edit::Undo] {
        let mut doc = draft();
        let why = edit.unsupported();
        doc.ops.push(Op { id: OpId(1), edit });
        assert_eq!(validate(&doc, &limits), Err(why));
    }
}

#[test]
fn field_writes_refuse_invalid_values() {
    let limits = Limits::default();
    let invalid = [
        (
            set(1, BlockKind::Weapons, 0, "unknown", Value::Int(0)),
            "unknown placement field",
        ),
        (
            set(1, BlockKind::Weapons, 0, "name index", Value::Int(0)),
            "read-only",
        ),
        (
            set(
                1,
                BlockKind::Weapons,
                0,
                "scale",
                Value::Float(F32::new(-1.0).unwrap()),
            ),
            "out of range",
        ),
        (
            set(1, BlockKind::Weapons, 0, "rotation", Value::Int(0)),
            "wrong type",
        ),
        (
            set(1, BlockKind::Weapons, 0, "palette index", Value::Int(0)),
            "wrong type",
        ),
        (
            set(
                1,
                BlockKind::Weapons,
                0,
                "palette index",
                Value::Tag(Some(key("vehi"))),
            ),
            "tag group",
        ),
        (
            set(1, BlockKind::Weapons, 0, "placement flags", Value::Int(-1)),
            "out of range",
        ),
        (
            set(1, BlockKind::NetgameFlags, 0, "type", Value::Int(65_536)),
            "out of range",
        ),
        (
            set(1, BlockKind::NetgameEquipment, 0, "type", Value::Int(256)),
            "out of range",
        ),
        (
            set(
                1,
                BlockKind::NetgameEquipment,
                0,
                "respawn time",
                Value::Int(32_768),
            ),
            "out of range",
        ),
        (
            set(
                1,
                BlockKind::NetgameEquipment,
                0,
                "item",
                Value::Tag(Some(key("shad"))),
            ),
            "tag group",
        ),
        (
            set(
                1,
                BlockKind::Weapons,
                u32::MAX,
                "position",
                Value::Vec3([F32::new(0.0).unwrap(); 3]),
            ),
            "index exceeds",
        ),
        (
            Op {
                id: OpId(1),
                edit: Edit::Placement(Placement::Add {
                    block: BlockKind::Weapons,
                    like: Some(128),
                    fields: vec![],
                }),
            },
            "index exceeds",
        ),
        (
            Op {
                id: OpId(1),
                edit: Edit::Placement(Placement::Remove {
                    block: BlockKind::Weapons,
                    index: 128,
                }),
            },
            "index exceeds",
        ),
    ];
    for (item, expected) in invalid {
        let mut doc = draft();
        doc.ops.push(item);
        refuse(&doc, &limits, expected);
    }

    let write = tag_write();
    let invalid = [
        (
            TagWrite {
                tag: key("shader"),
                ..write.clone()
            },
            "tag group",
        ),
        (
            TagWrite {
                tag: key("    "),
                ..write.clone()
            },
            "tag group",
        ),
        (
            TagWrite {
                tag: TagKey {
                    path: "a\\..\\b".into(),
                    ..key("shad")
                },
                ..write.clone()
            },
            "tag path",
        ),
        (
            TagWrite {
                tag: TagKey {
                    path: "a".repeat(257),
                    ..key("shad")
                },
                ..write.clone()
            },
            "tag path",
        ),
        (
            TagWrite {
                path: "".into(),
                ..write.clone()
            },
            "field path",
        ),
        (
            TagWrite {
                path: "colors[-1].color".into(),
                ..write.clone()
            },
            "invalid array index",
        ),
        (
            TagWrite {
                path: "colors[4294967296]".into(),
                ..write.clone()
            },
            "invalid array index",
        ),
        (
            TagWrite {
                path: "a.".repeat(16) + "b",
                ..write.clone()
            },
            "field path depth",
        ),
        (
            TagWrite {
                path: "a".repeat(257),
                ..write.clone()
            },
            "field path",
        ),
        (
            TagWrite {
                value: Value::Int(i64::MIN),
                ..write.clone()
            },
            "integer range",
        ),
        (
            TagWrite {
                value: Value::Int((1_i64 << 53) + 1),
                ..write.clone()
            },
            "integer range",
        ),
        (
            TagWrite {
                value: Value::Text("x".repeat(513)),
                ..write
            },
            "field text",
        ),
    ];
    for (write, expected) in invalid {
        let mut doc = draft();
        doc.ops.push(Op {
            id: OpId(1),
            edit: Edit::TagField(write),
        });
        refuse(&doc, &limits, expected);
    }

    let mut doc = draft();
    doc.ops.push(Op {
        id: OpId(1),
        edit: Edit::Placement(Placement::Add {
            block: BlockKind::Weapons,
            like: None,
            fields: [0, 1]
                .map(|i| FieldWrite {
                    field: "placement flags".into(),
                    value: Value::Int(i),
                })
                .to_vec(),
        }),
    });
    refuse(&doc, &limits, "listed twice");
}

#[test]
fn world_context_counts_and_budget_refuse_before_replay() {
    let mut doc = draft();
    let mut limits = Limits {
        base_counts: [(BlockKind::Weapons, 1)].into(),
        world: Some(WorldBounds {
            min: [-2.0; 3],
            max: [2.0; 3],
        }),
        ..Limits::default()
    };
    doc.ops.push(set(
        1,
        BlockKind::Weapons,
        0,
        "position",
        Value::Vec3([F32::new(2.0).unwrap(); 3]),
    ));
    validate(&doc, &limits).unwrap();
    doc.ops.push(set(
        2,
        BlockKind::Weapons,
        0,
        "position",
        Value::Vec3([F32::new(2.01).unwrap(); 3]),
    ));
    refuse(&doc, &limits, "outside the BSP");
    doc.ops.pop();
    limits.world = Some(WorldBounds {
        min: [3.0; 3],
        max: [2.0; 3],
    });
    refuse(&doc, &limits, "world bounds");
    limits.world = None;

    doc.ops.push(Op {
        id: OpId(2),
        edit: Edit::Placement(Placement::Remove {
            block: BlockKind::Weapons,
            index: 0,
        }),
    });
    doc.ops.push(set(
        3,
        BlockKind::Weapons,
        0,
        "placement flags",
        Value::Int(0),
    ));
    refuse(&doc, &limits, "does not exist");
    doc.ops.pop();
    doc.ops.push(add(3, BlockKind::Weapons));
    limits.max_adds = 0;
    refuse(&doc, &limits, "placement adds");
    limits.max_adds = 1;
    validate(&doc, &limits).unwrap();

    let mut doc = draft();
    doc.ops = (1..=100)
        .map(|i| add(i, BlockKind::NetgameEquipment))
        .collect();
    validate(&doc, &Limits::default()).unwrap();
    doc.ops.push(add(101, BlockKind::NetgameEquipment));
    refuse(&doc, &Limits::default(), "netgame_equipment");
    doc.ops.clear();
    limits.base_counts.insert(BlockKind::NetgameEquipment, 101);
    refuse(&doc, &limits, "netgame_equipment");
}

#[test]
fn file_caps_apply_before_using_hostile_payloads() {
    let limits = Limits::default();
    let large = vec![b' '; MAX_DOC_BYTES + 1];
    assert_eq!(
        ForgeDoc::from_bytes(&large, &limits),
        Err(Refusal::limit("document bytes", MAX_DOC_BYTES))
    );
    assert!(F32::new(f32::INFINITY).is_err());
    assert!(F32::new(f32::NAN).is_err());
    for payload in [
        ONE_OP.replace("\"80000000\"", "\"7fc00000\""),
        ONE_OP.replace("\"80000000\"", "\"7f800000\""),
        ONE_OP.replace("\"80000000\"", "\"FFFFFFFF\""),
        ONE_OP.replace("\"80000000\"", "\"0\""),
        ONE_OP.replace("\"80000000\"", "1.5"),
        ONE_OP.replace("\"weapons\"", "\"bogus\""),
        ONE_OP.replace("\"index\":0", "\"index\":0,\"index\":1"),
        ONE_OP.replace("\"index\":0", "\"index\":0,\"bytes\":[1,2,3]"),
        ONE_OP.replace("\"format\":1", "\"format\":1,\"blob\":\"aGk=\""),
        ONE_OP.replace(
            "{\"float\":\"80000000\"}",
            &("[".repeat(512) + &"]".repeat(512)),
        ),
        format!("{ONE_OP}{{}}"),
    ] {
        assert!(matches!(
            ForgeDoc::from_bytes(payload.as_bytes(), &limits),
            Err(Refusal::Json(_))
        ));
    }

    let mut doc = draft();
    doc.ops = (1..=MAX_OPS as u32)
        .map(|i| {
            let mut write = tag_write();
            write.tag.path = "a".repeat(256);
            write.value = Value::Text("b".repeat(512));
            Op {
                id: OpId(i),
                edit: Edit::TagField(write),
            }
        })
        .collect();
    assert_eq!(
        doc.id(),
        Err(Refusal::limit("document bytes", MAX_DOC_BYTES))
    );
    assert_eq!(
        validate(&doc, &limits),
        Err(Refusal::limit("document bytes", MAX_DOC_BYTES))
    );
}

#[test]
fn array_caps_cover_the_wire_and_in_memory_documents() {
    let limits = Limits::default();
    let mut doc = draft();
    let check = |what: &str, max: usize, doc: &ForgeDoc| {
        assert_eq!(validate(doc, &limits), Err(Refusal::limit(what, max)));
        let wire = serde_json::to_vec(doc).unwrap();
        let error = ForgeDoc::from_bytes(&wire, &limits).unwrap_err();
        assert!(error.to_string().contains("at most"), "{what}: {error}");
    };

    doc.sources = vec![doc.base.clone(); MAX_SOURCES + 1];
    check("sources", MAX_SOURCES, &doc);
    doc.sources.clear();
    doc.recipes = vec![Recipe::Import { slot: ImportId(1) }; MAX_RECIPES + 1];
    check("recipes", MAX_RECIPES, &doc);
    doc.recipes.clear();
    doc.imports = vec![
        ImportSlot {
            id: ImportId(1),
            name: "Prop".into(),
            sha256: Sha256([3; 32]),
            chassis: None
        };
        MAX_IMPORTS + 1
    ];
    check("import slots", MAX_IMPORTS, &doc);
    doc.imports.clear();
    doc.ops = vec![scale(); MAX_OPS + 1];
    check("operations", MAX_OPS, &doc);
    doc.ops = vec![scale()];
    assert_eq!(
        validate(
            &doc,
            &Limits {
                max_ops: 0,
                ..limits.clone()
            }
        ),
        Err(Refusal::limit("operations", 0))
    );
    doc.ops.clear();

    let write = tag_write();
    doc.recipes = vec![Recipe::Variant {
        tag: write.tag.clone(),
        writes: vec![write; MAX_FIELDS + 1],
    }];
    check("variant writes", MAX_FIELDS, &doc);
    doc.recipes = vec![Recipe::Generate {
        generator: "box".into(),
        parameters: vec![
            FieldWrite {
                field: "length".into(),
                value: Value::Int(1)
            };
            MAX_FIELDS + 1
        ],
    }];
    check("generator parameters", MAX_FIELDS, &doc);
    doc.recipes.clear();
    doc.ops = vec![Op {
        id: OpId(1),
        edit: Edit::Placement(Placement::Add {
            block: BlockKind::Weapons,
            like: None,
            fields: vec![
                FieldWrite {
                    field: "placement flags".into(),
                    value: Value::Int(1)
                };
                MAX_FIELDS + 1
            ],
        }),
    }];
    check("placement fields", MAX_FIELDS, &doc);
}

#[test]
fn seeded_fuzz_of_10000_hostile_documents() {
    let limits = Limits::default();
    let mut rng = Rng(0x1b78_01d5_693e_a235);
    let mut accepted = 0;
    for case in 0..10_000 {
        let bytes = match case % 4 {
            0 => {
                let len = rng.below(512);
                (0..len).map(|_| rng.next() as u8).collect()
            }
            1 => {
                let mut bytes = ONE_OP.as_bytes().to_vec();
                for _ in 0..rng.below(24) {
                    let pos = rng.below(bytes.len());
                    bytes[pos] = rng.next() as u8;
                }
                bytes
            }
            2 => {
                let mut doc = draft();
                for id in 1..=rng.below(20) as u32 {
                    let block = match rng.below(3) {
                        0 => BlockKind::Weapons,
                        1 => BlockKind::NetgameFlags,
                        _ => BlockKind::NetgameEquipment,
                    };
                    let index = rng.below(300) as u32;
                    doc.ops.push(match rng.below(3) {
                        0 => add(id, block),
                        1 => Op {
                            id: OpId(id),
                            edit: Edit::Placement(Placement::Remove { block, index }),
                        },
                        _ => set(
                            id,
                            block,
                            index,
                            "scale",
                            Value::Float(F32::new(rng.below(5) as f32).unwrap()),
                        ),
                    });
                }
                serde_json::to_vec(&doc).unwrap()
            }
            _ => {
                let end = rng.below(ONE_OP.len());
                ONE_OP.as_bytes()[..end].to_vec()
            }
        };
        if let Ok(doc) = ForgeDoc::from_bytes(&bytes, &limits) {
            accepted += 1;
            let json = doc.canonical_json().unwrap();
            assert_eq!(
                ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap(),
                doc,
                "seeded case {case}"
            );
        }
    }
    assert!((1..10_000).contains(&accepted));
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
