use std::collections::BTreeMap;

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const WEAPONS: BlockKind = BlockKind::Weapons;

/// The print of netgame flag fields `(1, 2, 3)`, facing 0.5, type 11,
/// team 8, identifier 3.
const FLAG_PRINT: &str = "1373ae4eb8ca684d";
/// A print no test map gives: documents need no map to validate.
const OTHER_PRINT: &str = "0123456789abcdef";

/// Format 3: a set of flag 0's team, a reset of flag 0 and a removal of
/// flag 1, each named by print.
const ENTRIES: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":3,"imports":[],"ops":[{"edit":{"entry":{"set":{"target":{"base":{"block":"netgame_flags","index":0,"print":"1373ae4eb8ca684d"}},"write":{"field":"team","value":{"int":1}}}}},"id":1},{"edit":{"entry":{"reset":{"target":{"base":{"block":"netgame_flags","index":0,"print":"1373ae4eb8ca684d"}}}}},"id":2},{"edit":{"entry":{"remove":{"target":{"base":{"block":"netgame_flags","index":1,"print":"0123456789abcdef"}}}}},"id":3}],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;

fn f(x: f32) -> F32 {
    F32::new(x).unwrap()
}

fn parse(hex: &str) -> Print {
    serde_json::from_value(serde_json::Value::String(hex.into())).unwrap()
}

fn printed(block: BlockKind, index: u32, print: &str) -> Endpoint {
    Endpoint::Base {
        block,
        index,
        print: Some(parse(print)),
    }
}

fn flag(index: u32) -> Endpoint {
    printed(
        FLAGS,
        index,
        if index == 0 { FLAG_PRINT } else { OTHER_PRINT },
    )
}

fn flag_fields() -> Vec<Value> {
    vec![
        Value::Vec3([f(1.0), f(2.0), f(3.0)]),
        Value::Float(f(0.5)),
        Value::Int(11),
        Value::Int(8),
        Value::Int(3),
    ]
}

fn weapon_fields(palette: Option<TagKey>) -> Vec<Value> {
    vec![
        Value::Tag(palette),
        Value::Int(-1),
        Value::Int(0),
        Value::Vec3([f(1.5), f(-2.0), f(0.25)]),
        Value::Vec3([f(0.0); 3]),
        Value::Float(f(0.0)),
    ]
}

fn entry(id: u32, edit: EntryOp) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(edit),
    }
}

fn set_entry(id: u32, target: Endpoint, field: &str, value: Value) -> Op {
    entry(
        id,
        EntryOp::Set {
            target,
            write: FieldWrite {
                field: field.into(),
                value,
            },
        },
    )
}

fn remove_entry(id: u32, target: Endpoint) -> Op {
    entry(id, EntryOp::Remove { target })
}

fn reset_entry(id: u32, target: Endpoint) -> Op {
    entry(id, EntryOp::Reset { target })
}

fn forge(id: u32, edit: Forge) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(edit),
    }
}

fn teleporter(id: u32, sender: Endpoint, receiver: Endpoint) -> Op {
    forge(
        id,
        Forge::Link(Link::Teleporter {
            sender,
            receiver,
            channel: 1,
        }),
    )
}

fn label(id: u32, target: Endpoint) -> Op {
    forge(
        id,
        Forge::Label {
            target,
            label: Some(Label::new("goal", None).unwrap()),
        },
    )
}

/// A document of `ops` at the format they need.
fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn flags(count: u32) -> Limits {
    Limits {
        base_counts: BTreeMap::from([(FLAGS, count), (WEAPONS, 2)]),
        ..Limits::default()
    }
}

fn refuse(ops: Vec<Op>, limits: &Limits, expected: &str) {
    let error = validate(&with(ops.clone()), limits).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "{ops:?}: {error} did not mention {expected}"
    );
}

#[test]
fn print_golden_vectors() {
    assert_eq!(
        print(FLAGS, &flag_fields()).unwrap().to_string(),
        FLAG_PRINT
    );
    assert_eq!(
        print(WEAPONS, &weapon_fields(Some(key("weap"))))
            .unwrap()
            .to_string(),
        "5026c8b19ee52e59"
    );
    // A palette index of none is `Tag(None)`, and the print differs.
    assert_eq!(
        print(WEAPONS, &weapon_fields(None)).unwrap().to_string(),
        "7cf3b92e7bb9396c"
    );
    // The block's name is in the digest.
    assert_ne!(
        print(BlockKind::Scenery, &weapon_fields(None)).unwrap(),
        print(WEAPONS, &weapon_fields(None)).unwrap()
    );
    // One field's value changes the print.
    let mut moved = flag_fields();
    moved[0] = Value::Vec3([f(1.0), f(2.0), f(3.5)]);
    assert_ne!(print(FLAGS, &moved).unwrap(), parse(FLAG_PRINT));
}

#[test]
fn a_print_covers_every_typed_field_with_its_type() {
    let mut short = flag_fields();
    short.pop();
    assert!(
        print(FLAGS, &short)
            .unwrap_err()
            .to_string()
            .contains("covers the layout's 5 fields")
    );
    let mut wrong = flag_fields();
    wrong[2] = Value::Float(f(11.0));
    assert!(
        print(FLAGS, &wrong)
            .unwrap_err()
            .to_string()
            .contains("type")
    );
    let mut range = flag_fields();
    range[3] = Value::Int(-1); // team is a u16
    assert!(print(FLAGS, &range).is_err());
    // Read-only fields count: the weapon's name index is read-only.
    let mut name = weapon_fields(None);
    name[1] = Value::Float(f(-1.0));
    assert!(print(WEAPONS, &name).is_err());
}

#[test]
fn a_print_is_sixteen_lowercase_hex_digits() {
    let p = parse(FLAG_PRINT);
    assert_eq!(
        serde_json::to_string(&p).unwrap(),
        format!("\"{FLAG_PRINT}\"")
    );
    for bad in [
        "1373AE4EB8CA684D",
        "1373ae4eb8ca684",
        "1373ae4eb8ca684d0",
        "1373ae4eb8ca684g",
        "",
    ] {
        assert!(
            serde_json::from_value::<Print>(serde_json::Value::String(bad.into())).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn the_print_check_refuses_another_entry_by_block_and_index() {
    check_print(FLAGS, 4, parse(FLAG_PRINT), &flag_fields()).unwrap();
    let mut other = flag_fields();
    other[4] = Value::Int(4);
    let refusal = check_print(FLAGS, 4, parse(FLAG_PRINT), &other).unwrap_err();
    assert_eq!(
        refusal,
        Refusal::Print {
            block: FLAGS,
            index: 4
        }
    );
    assert_eq!(
        refusal.to_string(),
        "the map's netgame_flags entry 4 is not the one this edit was made on"
    );
    assert_eq!(
        serde_json::to_string(&refusal).unwrap(),
        r#"{"print":{"block":"netgame_flags","index":4}}"#
    );
}

#[test]
fn format_3_golden_vector() {
    let doc = with(vec![
        set_entry(1, flag(0), "team", Value::Int(1)),
        reset_entry(2, flag(0)),
        remove_entry(3, flag(1)),
    ]);
    assert_eq!(doc.format, FORMAT_ENTRIES);
    assert_eq!(FORMAT_ENTRIES, 3);
    assert_eq!(doc.canonical_json().unwrap(), ENTRIES);
    assert_eq!(
        doc.id().unwrap().to_string(),
        "be1eea848c1c67bdab49fdb7bfdaeb3c0a0eb98f0db07d9b99daaa151181795a"
    );
    assert_eq!(
        ForgeDoc::from_bytes(ENTRIES.as_bytes(), &Limits::default()).unwrap(),
        doc
    );

    // One spelling: an entry document at an older format is refused, and
    // so is a document without entry ops at format 3.
    for old in [FORMAT, FORMAT_LABELS] {
        let bytes = ENTRIES.replace("\"format\":3", &format!("\"format\":{old}"));
        assert_eq!(
            ForgeDoc::from_bytes(bytes.as_bytes(), &Limits::default()),
            Err(Refusal::Format(old))
        );
    }
    let mut plain = draft();
    plain.format = FORMAT_ENTRIES;
    assert_eq!(
        validate(&plain, &Limits::default()),
        Err(Refusal::Format(FORMAT_ENTRIES))
    );
}

#[test]
fn documents_without_entry_ops_keep_their_format_and_identity() {
    let plain = ForgeDoc::from_bytes(EMPTY.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(plain.required_format(), FORMAT);
    assert_eq!(
        plain.id().unwrap().to_string(),
        "77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001"
    );
    let one = ForgeDoc::from_bytes(ONE_OP.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(one.format, FORMAT);
    assert_eq!(
        one.id().unwrap().to_string(),
        "7a64929ecc49d0a2f743128ee53371f8d3aed410e7f70fbfd1b2858551cdba6d"
    );

    // A link or label without a print keeps the spelling it had: no
    // `print` member, and its format.
    let bare = Endpoint::base(FLAGS, 0);
    assert_eq!(
        serde_json::to_string(&bare).unwrap(),
        r#"{"base":{"block":"netgame_flags","index":0}}"#
    );
    assert_eq!(
        serde_json::from_str::<Endpoint>(r#"{"base":{"block":"netgame_flags","index":0}}"#)
            .unwrap(),
        bare
    );
    assert_eq!(with(vec![label(1, bare)]).format, FORMAT_LABELS);
    assert_eq!(
        with(vec![teleporter(1, bare, Endpoint::base(FLAGS, 1))]).format,
        FORMAT
    );

    // With a print it is format 3.
    assert_eq!(with(vec![label(1, flag(0))]).format, FORMAT_ENTRIES);
    assert_eq!(
        with(vec![teleporter(1, flag(0), Endpoint::base(FLAGS, 1))]).format,
        FORMAT_ENTRIES
    );
}

#[test]
fn entry_ops_have_one_canonical_spelling() {
    let added = Endpoint::Added(OpId(1));
    let cases = [
        (
            set_entry(2, added, "scale", Value::Float(f(2.0))),
            r#"{"edit":{"entry":{"set":{"target":{"added":1},"write":{"field":"scale","value":{"float":"40000000"}}}}},"id":2}"#,
        ),
        (
            remove_entry(3, added),
            r#"{"edit":{"entry":{"remove":{"target":{"added":1}}}},"id":3}"#,
        ),
        (
            reset_entry(4, printed(WEAPONS, 0, OTHER_PRINT)),
            r#"{"edit":{"entry":{"reset":{"target":{"base":{"block":"weapons","index":0,"print":"0123456789abcdef"}}}}},"id":4}"#,
        ),
    ];
    for (op, json) in cases {
        assert_eq!(
            crate::wire::canonical(&op, "op", MAX_DOC_BYTES).unwrap(),
            json
        );
        assert_eq!(serde_json::from_str::<Op>(json).unwrap(), op);
    }
    // Unknown members are refused.
    assert!(
        serde_json::from_str::<Op>(
            r#"{"edit":{"entry":{"remove":{"target":{"added":1},"index":0}}},"id":3}"#
        )
        .is_err()
    );
}

#[test]
fn accepted_entry_edits() {
    let limits = flags(3);
    let ok = |ops: Vec<Op>| validate(&with(ops), &limits).unwrap();
    // Each kind of op on a base entry, and on an added one.
    ok(vec![set_entry(1, flag(0), "team", Value::Int(1))]);
    ok(vec![
        set_entry(1, flag(0), "position", Value::Vec3([f(4.0); 3])),
        set_entry(2, flag(0), "facing", Value::Float(f(1.0))),
        remove_entry(3, flag(0)),
        reset_entry(4, flag(0)),
        set_entry(5, flag(0), "identifier", Value::Int(2)),
    ]);
    ok(vec![
        add(1, WEAPONS),
        set_entry(2, Endpoint::Added(OpId(1)), "scale", Value::Float(f(2.0))),
        remove_entry(3, Endpoint::Added(OpId(1))),
    ]);
    // A palette change names a tag of the palette's group.
    ok(vec![set_entry(
        1,
        printed(WEAPONS, 1, OTHER_PRINT),
        "palette index",
        Value::Tag(Some(key("weap"))),
    )]);
    // A reset puts a removed entry back, so it can be linked again.
    ok(vec![
        remove_entry(1, flag(0)),
        reset_entry(2, flag(0)),
        teleporter(3, flag(0), flag(1)),
    ]);
    // Without the map's counts the same rules hold where they can.
    validate(
        &with(vec![remove_entry(1, flag(7)), reset_entry(2, flag(7))]),
        &Limits::default(),
    )
    .unwrap();
}

#[test]
fn every_entry_rule_has_a_refusing_case() {
    let limits = Limits {
        world: Some(WorldBounds {
            min: [-10.0; 3],
            max: [10.0; 3],
        }),
        ..flags(3)
    };
    let added = Endpoint::Added(OpId(1));
    let bare = Endpoint::base(FLAGS, 0);
    let cases: Vec<(Vec<Op>, &str)> = vec![
        // The target.
        (
            vec![set_entry(1, bare, "team", Value::Int(1))],
            "named with its print",
        ),
        (vec![remove_entry(1, bare)], "named with its print"),
        (vec![reset_entry(1, bare)], "named with its print"),
        (
            vec![set_entry(
                1,
                printed(WEAPONS, 128, OTHER_PRINT),
                "scale",
                Value::Float(f(1.0)),
            )],
            "exceeds block limit",
        ),
        (
            vec![remove_entry(1, Endpoint::Added(OpId(0)))],
            "want the ID of an add op",
        ),
        (
            vec![set_entry(1, flag(3), "team", Value::Int(1))],
            "names no netgame_flags entry",
        ),
        (
            vec![set_entry(1, added, "scale", Value::Float(f(1.0)))],
            "names no added entry",
        ),
        // The field and its value, on a base and an added target.
        (
            vec![set_entry(1, flag(0), "scale", Value::Float(f(1.0)))],
            "unknown placement field",
        ),
        (
            vec![add(1, WEAPONS), set_entry(2, added, "team", Value::Int(1))],
            "unknown placement field",
        ),
        (
            vec![set_entry(
                1,
                printed(WEAPONS, 0, OTHER_PRINT),
                "name index",
                Value::Int(2),
            )],
            "read-only",
        ),
        (
            vec![set_entry(1, flag(0), "team", Value::Float(f(1.0)))],
            "wrong type or value out of range",
        ),
        (
            vec![set_entry(1, flag(0), "team", Value::Int(70_000))],
            "wrong type or value out of range",
        ),
        (
            vec![
                add(1, WEAPONS),
                set_entry(2, added, "scale", Value::Float(f(-1.0))),
            ],
            "wrong type or value out of range",
        ),
        (
            vec![set_entry(
                1,
                printed(WEAPONS, 0, OTHER_PRINT),
                "palette index",
                Value::Tag(Some(key("vehi"))),
            )],
            "tag group is not accepted here",
        ),
        (
            vec![set_entry(
                1,
                flag(0),
                "position",
                Value::Vec3([f(11.0), f(0.0), f(0.0)]),
            )],
            "outside the BSP bounds",
        ),
        (
            vec![
                add(1, WEAPONS),
                set_entry(
                    2,
                    added,
                    "position",
                    Value::Vec3([f(0.0), f(0.0), f(-11.0)]),
                ),
            ],
            "outside the BSP bounds",
        ),
        // A removed target.
        (
            vec![
                remove_entry(1, flag(0)),
                set_entry(2, flag(0), "team", Value::Int(1)),
            ],
            "names a removed entry",
        ),
        (
            vec![remove_entry(1, flag(0)), remove_entry(2, flag(0))],
            "names a removed entry",
        ),
        (
            vec![
                add(1, WEAPONS),
                remove_entry(2, added),
                set_entry(3, added, "scale", Value::Float(f(1.0))),
            ],
            "names a removed entry",
        ),
        (
            // Running index 0 of three flags swap-removes base flag 0.
            vec![
                Op {
                    id: OpId(1),
                    edit: Edit::Placement(Placement::Remove {
                        block: FLAGS,
                        index: 0,
                    }),
                },
                set_entry(2, flag(0), "team", Value::Int(1)),
            ],
            "names a removed entry",
        ),
        (
            vec![remove_entry(1, flag(0)), teleporter(2, flag(0), flag(1))],
            "names no netgame_flags entry",
        ),
        // A linked or labelled entry.
        (
            vec![teleporter(1, flag(0), flag(1)), remove_entry(2, flag(0))],
            "remove the entry's links first",
        ),
        (
            vec![label(1, bare), remove_entry(2, flag(0))],
            "clear the entry's label first",
        ),
        // Reset.
        (
            vec![reset_entry(1, flag(0))],
            "the entry has no edit to reset",
        ),
        (
            vec![
                set_entry(1, flag(0), "team", Value::Int(1)),
                reset_entry(2, flag(0)),
                reset_entry(3, flag(0)),
            ],
            "the entry has no edit to reset",
        ),
        (
            vec![add(1, WEAPONS), reset_entry(2, added)],
            "a reset names a base entry",
        ),
        // One print an entry.
        (
            vec![
                set_entry(1, flag(0), "team", Value::Int(1)),
                set_entry(2, printed(FLAGS, 0, OTHER_PRINT), "team", Value::Int(2)),
            ],
            "two different prints",
        ),
        (
            vec![
                label(1, printed(FLAGS, 0, OTHER_PRINT)),
                remove_entry(2, flag(0)),
            ],
            "two different prints",
        ),
    ];
    for (ops, expected) in cases {
        refuse(ops, &limits, expected);
    }
}

#[test]
fn a_log_entry_with_an_entry_op_is_checked_on_its_own() {
    let bare = Proposal {
        author: "Editor".into(),
        base_seq: 0,
        op: remove_entry(1, Endpoint::base(FLAGS, 0)),
        was: None,
    };
    let json = serde_json::to_vec(&bare).unwrap();
    let error = Proposal::from_bytes(&json, &Limits::default()).unwrap_err();
    assert!(
        error.to_string().contains("named with its print"),
        "{error}"
    );

    let printed = Proposal {
        op: remove_entry(1, flag(0)),
        ..bare
    };
    let json = serde_json::to_vec(&printed).unwrap();
    assert_eq!(
        Proposal::from_bytes(&json, &Limits::default()).unwrap(),
        printed
    );
}

#[test]
fn removal_and_reset_resolve_final_indices() {
    let counts = BTreeMap::from([(FLAGS, 3)]);
    // Removing base flag 0 moves flag 2 into its row.
    let doc = with(vec![
        remove_entry(1, flag(0)),
        teleporter(2, flag(2), flag(1)),
    ]);
    let links = doc.links(&counts).unwrap();
    assert_eq!(links.active[0].indices, [0, 1]);
    // Links resolve by identity: the stored link has no print.
    assert_eq!(
        links.active[0].link,
        Link::Teleporter {
            sender: Endpoint::base(FLAGS, 2),
            receiver: Endpoint::base(FLAGS, 1),
            channel: 1,
        }
    );

    // A reset leaves the removal out, so every flag keeps its row.
    let doc = with(vec![
        remove_entry(1, flag(0)),
        reset_entry(2, flag(0)),
        teleporter(3, flag(2), flag(1)),
    ]);
    assert_eq!(doc.links(&counts).unwrap().active[0].indices, [2, 1]);

    // An added entry's removal swap-removes it as a placement removal does.
    let doc = with(vec![
        add(1, FLAGS),
        add(2, FLAGS),
        remove_entry(3, Endpoint::Added(OpId(1))),
        teleporter(4, Endpoint::Added(OpId(2)), flag(1)),
    ]);
    assert_eq!(doc.links(&counts).unwrap().active[0].indices, [3, 1]);
}

#[test]
fn a_fold_of_entry_ops_sets_the_format_and_undo_restores_it() {
    let doc = draft();
    let limits = flags(3);
    let mut at = LogHead::genesis(&doc).unwrap();
    let mut entries = Vec::new();
    for op in [
        remove_entry(1, flag(0)),
        Op {
            id: OpId(1),
            edit: Edit::Undo,
        },
        set_entry(2, flag(1), "team", Value::Int(2)),
    ] {
        let entry = at
            .stamp(
                at.tick + 1,
                Proposal {
                    author: "Editor".into(),
                    base_seq: at.seq,
                    op,
                    was: None,
                },
                None,
                &limits,
            )
            .unwrap();
        at = LogHead {
            seq: entry.seq,
            tick: entry.tick,
            hash: entry.hash().unwrap(),
        };
        entries.push(entry);
    }
    let (after, _) = fold_from(&doc, LogHead::genesis(&doc).unwrap(), &entries, &limits).unwrap();
    assert_eq!(after.format, FORMAT_ENTRIES);
    assert_eq!(
        after.ops,
        vec![set_entry(2, flag(1), "team", Value::Int(2))]
    );

    // Undoing the last entry op returns the document to format 1.
    let (folded, _) = fold_from(
        &doc,
        LogHead::genesis(&doc).unwrap(),
        &entries[..2],
        &limits,
    )
    .unwrap();
    assert_eq!(folded, doc);
}

#[test]
fn replay_refuses_entry_ops_until_they_lower() {
    struct Nothing;
    impl Apply for Nothing {
        fn placement(&mut self, _: &Placement) -> Result<()> {
            Ok(())
        }
        fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
            Ok(())
        }
    }
    let doc = with(vec![set_entry(1, flag(0), "team", Value::Int(1))]);
    assert_eq!(
        replay(&doc, &Limits::default(), &mut Nothing),
        Err(Refusal::Unsupported("entry".into()))
    );
}

#[test]
fn seeded_fuzz_of_entry_documents() {
    let limits = flags(4);
    let mut rng = Rng(0x5eed_e17e_0003_f04e);
    let prints = [FLAG_PRINT, OTHER_PRINT];
    let fields: [(&str, fn(&mut Rng) -> Value); 5] = [
        ("team", |r| Value::Int(r.below(10) as i64 - 1)),
        ("facing", |r| {
            Value::Float(F32::new(r.below(7) as f32).unwrap())
        }),
        ("identifier", |r| Value::Int(r.below(5) as i64)),
        ("scale", |r| {
            Value::Float(F32::new(r.below(3) as f32).unwrap())
        }),
        ("position", |r| {
            Value::Vec3([F32::new(r.below(4) as f32).unwrap(); 3])
        }),
    ];
    let mut accepted = 0;
    let mut entry_docs = 0;
    for case in 0..4_000 {
        let bytes = match case % 3 {
            0 => {
                let mut bytes = ENTRIES.as_bytes().to_vec();
                for _ in 0..rng.below(16) {
                    let pos = rng.below(bytes.len());
                    bytes[pos] = rng.next() as u8;
                }
                bytes
            }
            _ => {
                let mut doc = draft();
                for id in 1..=rng.below(12) as u32 + 1 {
                    let target = match rng.below(4) {
                        0 => Endpoint::Added(OpId(rng.below(id as usize + 1) as u32)),
                        1 => Endpoint::base(FLAGS, rng.below(5) as u32),
                        _ => printed(FLAGS, rng.below(5) as u32, prints[rng.below(2)]),
                    };
                    doc.ops.push(match rng.below(6) {
                        0 => add(id, if rng.below(2) == 0 { FLAGS } else { WEAPONS }),
                        1 => remove_entry(id, target),
                        2 => reset_entry(id, target),
                        3 => teleporter(id, target, printed(FLAGS, 3, OTHER_PRINT)),
                        _ => {
                            let (name, value) = fields[rng.below(fields.len())];
                            set_entry(id, target, name, value(&mut rng))
                        }
                    });
                }
                doc.format = match rng.below(4) {
                    0 => rng.below(5) as u32,
                    _ => doc.required_format(),
                };
                serde_json::to_vec(&doc).unwrap()
            }
        };
        if let Ok(doc) = ForgeDoc::from_bytes(&bytes, &limits) {
            accepted += 1;
            assert_eq!(doc.format, doc.required_format(), "seeded case {case}");
            if doc.ops.iter().any(|op| matches!(op.edit, Edit::Entry(_))) {
                entry_docs += 1;
                assert_eq!(doc.format, FORMAT_ENTRIES, "seeded case {case}");
            }
            let json = doc.canonical_json().unwrap();
            let again = ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap();
            assert_eq!(again, doc, "seeded case {case}");
            assert_eq!(again.id().unwrap(), doc.id().unwrap(), "seeded case {case}");
        }
    }
    assert!((1..4_000).contains(&accepted), "{accepted}");
    assert!(entry_docs > 0);
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
