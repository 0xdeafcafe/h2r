use std::collections::BTreeMap;

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const WEAPONS: BlockKind = BlockKind::Weapons;

/// Format 2, one label op: `"checkpoint" 3` on the base map's netgame flag 0.
const LABELLED: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":2,"imports":[],"ops":[{"edit":{"forge":{"label":{"label":{"number":3,"text":"checkpoint"},"target":{"base":{"block":"netgame_flags","index":0}}}}},"id":1}],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;

fn base(block: BlockKind, index: u32) -> Endpoint {
    Endpoint::Base { block, index }
}

fn text(text: &str, number: Option<i32>) -> Label {
    Label {
        text: text.into(),
        number,
    }
}

fn set_label(id: u32, target: Endpoint, label: Label) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Label {
            target,
            label: Some(label),
        }),
    }
}

fn clear_label(id: u32, target: Endpoint) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Label {
            target,
            label: None,
        }),
    }
}

fn remove(id: u32, block: BlockKind, index: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Remove { block, index }),
    }
}

fn counts(flags: u32, weapons: u32) -> Limits {
    Limits {
        base_counts: BTreeMap::from([(FLAGS, flags), (WEAPONS, weapons)]),
        ..Limits::default()
    }
}

/// A document whose `format` is the one its ops need.
fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn refuse(ops: Vec<Op>, limits: &Limits, expected: &str) {
    let error = validate(&with(ops), limits).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "{error} did not mention {expected}"
    );
}

#[test]
fn label_text_is_short_lowercase_names() {
    let long = "x".repeat(32);
    for good in ["a", "ctf_flag_spawn", "hill-2", "zone.a", "0", long.as_str()] {
        Label::new(good, None).unwrap();
    }
    Label::new("checkpoint", Some(-3)).unwrap();
    let too_long = "x".repeat(33);
    for bad in [
        "",
        too_long.as_str(),
        "Upper",
        "has space",
        "tab\t",
        "line\nbreak",
        "nul\0",
        "caf\u{e9}",
        "slash/",
        "a:b",
    ] {
        let error = Label::new(bad, None).unwrap_err();
        assert!(error.to_string().contains("label"), "{bad:?}: {error}");
    }
    assert!(Label::new("", None).is_err());
}

#[test]
fn a_labelled_document_is_format_2_and_others_stay_format_1() {
    let doc = with(vec![set_label(
        1,
        base(FLAGS, 0),
        text("checkpoint", Some(3)),
    )]);
    assert_eq!(doc.format, FORMAT_LABELS);
    assert_eq!(doc.canonical_json().unwrap(), LABELLED);
    assert_eq!(
        doc.id().unwrap().to_string(),
        "0065ad439cdb50d04344ae18519b585f44d613d193e6dd2007437c86088125e4"
    );
    assert_eq!(
        ForgeDoc::from_bytes(LABELLED.as_bytes(), &Limits::default()).unwrap(),
        doc
    );

    // A document without labels is the format 1 document it always was.
    let plain = draft();
    assert_eq!(plain.required_format(), FORMAT);
    assert_eq!(
        ForgeDoc::from_bytes(EMPTY.as_bytes(), &Limits::default())
            .unwrap()
            .id()
            .unwrap()
            .to_string(),
        "77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001"
    );
    assert_eq!(
        ForgeDoc::from_bytes(ONE_OP.as_bytes(), &Limits::default())
            .unwrap()
            .format,
        FORMAT
    );

    // Each document has one spelling: the format is the one its ops need.
    let mut wrong = doc.clone();
    wrong.format = FORMAT;
    assert_eq!(
        validate(&wrong, &Limits::default()),
        Err(Refusal::Format(FORMAT))
    );
    let mut wrong = plain;
    wrong.format = FORMAT_LABELS;
    assert_eq!(
        validate(&wrong, &Limits::default()),
        Err(Refusal::Format(FORMAT_LABELS))
    );
    let old = LABELLED.replace("\"format\":2", "\"format\":1");
    assert_eq!(
        ForgeDoc::from_bytes(old.as_bytes(), &Limits::default()),
        Err(Refusal::Format(FORMAT))
    );
}

#[test]
fn a_clear_has_one_spelling_and_round_trips() {
    let op = clear_label(2, Endpoint::Added(OpId(1)));
    assert_eq!(
        crate::wire::canonical(&op, "op", MAX_DOC_BYTES).unwrap(),
        r#"{"edit":{"forge":{"label":{"target":{"added":1}}}},"id":2}"#
    );
    let doc = with(vec![
        add(1, WEAPONS),
        set_label(3, Endpoint::Added(OpId(1)), text("a", None)),
        op,
    ]);
    let json = doc.canonical_json().unwrap();
    assert_eq!(
        ForgeDoc::from_bytes(json.as_bytes(), &Limits::default()).unwrap(),
        doc
    );
    // An explicit null clears too, and reads back as the same clear.
    let null = json.replace(r#""label":{"target""#, r#""label":{"label":null,"target""#);
    assert_eq!(
        ForgeDoc::from_bytes(null.as_bytes(), &Limits::default()).unwrap(),
        doc
    );
}

#[test]
fn every_label_rule_has_a_refusing_case() {
    let limits = counts(2, 3);
    let cases: Vec<(Vec<Op>, &str)> = vec![
        (
            vec![set_label(1, base(FLAGS, 0), text("Bad", None))],
            "lowercase",
        ),
        (vec![set_label(1, base(FLAGS, 0), text("", None))], "1 to 32"),
        (
            vec![set_label(1, base(FLAGS, 40_000), text("a", None))],
            "block limit",
        ),
        (
            vec![set_label(1, Endpoint::Added(OpId(0)), text("a", None))],
            "ID of an add op",
        ),
        (
            vec![set_label(1, base(FLAGS, 2), text("a", None))],
            "names no entry",
        ),
        (
            vec![set_label(1, Endpoint::Added(OpId(9)), text("a", None))],
            "names no entry",
        ),
        (vec![clear_label(1, base(FLAGS, 0))], "no label to clear"),
        (
            vec![
                set_label(1, base(FLAGS, 0), text("a", None)),
                clear_label(2, base(FLAGS, 0)),
                clear_label(3, base(FLAGS, 0)),
            ],
            "no label to clear",
        ),
        (
            vec![
                set_label(1, base(FLAGS, 0), text("a", None)),
                remove(2, FLAGS, 0),
            ],
            "clear the entry's label first",
        ),
        (
            vec![
                remove(1, FLAGS, 0),
                set_label(2, base(FLAGS, 0), text("a", None)),
            ],
            // Entry 1 moved to index 0; entry 0 is gone.
            "names no entry",
        ),
        (
            vec![
                add(1, WEAPONS),
                remove(2, WEAPONS, 3),
                set_label(3, Endpoint::Added(OpId(1)), text("a", None)),
            ],
            "names no entry",
        ),
    ];
    for (ops, expected) in cases {
        refuse(ops, &limits, expected);
    }
}

#[test]
fn setting_replaces_and_clearing_frees_the_entry() {
    let limits = counts(2, 3);
    let ops = vec![
        set_label(1, base(FLAGS, 0), text("a", Some(1))),
        set_label(2, base(FLAGS, 0), text("b", None)),
        set_label(3, base(FLAGS, 1), text("b", None)),
        clear_label(4, base(FLAGS, 1)),
        // A cleared entry may be removed again.
        remove(5, FLAGS, 1),
    ];
    let doc = with(ops);
    validate(&doc, &limits).unwrap();
    let labels = doc.labels(&limits.base_counts).unwrap();
    // Labels need not be unique, and a placement keeps the last one set.
    assert_eq!(
        labels.active,
        vec![ActiveLabel {
            id: OpId(2),
            block: FLAGS,
            index: 0,
            label: text("b", None),
        }]
    );
}

#[test]
fn labels_follow_their_entries_through_appends_and_swap_removes() {
    let limits = counts(0, 3);
    let doc = with(vec![
        add(5, WEAPONS),
        set_label(6, base(WEAPONS, 2), text("zone", Some(1))),
        set_label(7, Endpoint::Added(OpId(5)), text("zone", Some(2))),
        // The last entry (the added one) moves into index 0.
        remove(8, WEAPONS, 0),
    ]);
    validate(&doc, &limits).unwrap();
    let labels = doc.labels(&limits.base_counts).unwrap();
    assert_eq!(
        labels.active,
        vec![
            ActiveLabel {
                id: OpId(7),
                block: WEAPONS,
                index: 0,
                label: text("zone", Some(2)),
            },
            ActiveLabel {
                id: OpId(6),
                block: WEAPONS,
                index: 2,
                label: text("zone", Some(1)),
            },
        ]
    );

    // The base count is needed only to resolve, not to validate.
    validate(&doc, &Limits::default()).unwrap();
    let error = doc.labels(&BTreeMap::new()).unwrap_err();
    assert!(error.to_string().contains("count is needed"), "{error}");
}

fn stamp_all(doc: &ForgeDoc, ops: &[Op]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut at = LogHead::genesis(doc).unwrap();
    for op in ops {
        let entry = at
            .stamp(
                at.tick + 1,
                Proposal {
                    author: "Editor".into(),
                    base_seq: at.seq,
                    op: op.clone(),
                    was: None,
                },
                None,
                &Limits::default(),
            )
            .unwrap();
        at = LogHead {
            seq: entry.seq,
            tick: entry.tick,
            hash: entry.hash().unwrap(),
        };
        entries.push(entry);
    }
    entries
}

#[test]
fn labels_go_through_the_edit_log_and_undo() {
    let doc = draft();
    let label = set_label(1, base(FLAGS, 0), text("start", None));
    let undo = inverse(&label, &Before::capture(&doc)).unwrap();
    assert_eq!(undo.edit, Edit::Undo);

    let entries = stamp_all(&doc, &[label.clone()]);
    let folded = fold(&doc, &entries).unwrap();
    assert_eq!(folded.format, FORMAT_LABELS);
    assert_eq!(folded.ops, vec![label.clone()]);
    folded.canonical_json().unwrap();

    // Retracting the only label returns the format 1 document.
    let entries = stamp_all(&doc, &[label.clone(), undo]);
    let folded = fold(&doc, &entries).unwrap();
    assert_eq!(folded, doc);
    assert_eq!(folded.format, FORMAT);

    // A label op the document cannot hold is refused when stamped.
    let at = LogHead::genesis(&doc).unwrap();
    let bad = set_label(1, base(FLAGS, 0), text("Bad", None));
    assert!(
        at.stamp(
            1,
            Proposal {
                author: "Editor".into(),
                base_seq: 0,
                op: bad,
                was: None,
            },
            None,
            &Limits::default(),
        )
        .is_err()
    );
}

#[derive(Default)]
struct Recorder {
    labels: Vec<Labels>,
    links: usize,
}

impl Apply for Recorder {
    fn placement(&mut self, _: &Placement) -> Result<()> {
        Ok(())
    }

    fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
        Ok(())
    }

    fn links(&mut self, _: &Links) -> Result<()> {
        self.links += 1;
        Ok(())
    }

    fn labels(&mut self, labels: &Labels) -> Result<()> {
        self.labels.push(labels.clone());
        Ok(())
    }
}

struct Plain;

impl Apply for Plain {
    fn placement(&mut self, _: &Placement) -> Result<()> {
        Ok(())
    }

    fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
        Ok(())
    }
}

#[test]
fn replay_hands_the_active_labels_to_the_adapter() {
    let limits = counts(2, 3);
    let doc = with(vec![
        add(1, FLAGS),
        set_label(2, base(FLAGS, 1), text("a", None)),
        set_label(3, Endpoint::Added(OpId(1)), text("b", Some(7))),
    ]);
    let mut recorder = Recorder::default();
    replay(&doc, &limits, &mut recorder).unwrap();
    assert_eq!(recorder.links, 0);
    assert_eq!(recorder.labels.len(), 1);
    let got: Vec<_> = recorder.labels[0]
        .active
        .iter()
        .map(|l| (l.index, l.label.text.as_str(), l.label.number))
        .collect();
    assert_eq!(got, vec![(1, "a", None), (2, "b", Some(7))]);

    // A document with no label op never calls the hook.
    let mut recorder = Recorder::default();
    replay(&with(vec![add(1, FLAGS)]), &limits, &mut recorder).unwrap();
    assert!(recorder.labels.is_empty());

    // An adapter without the hook refuses a label by name, and takes a
    // document whose labels were all cleared.
    let error = replay(&doc, &limits, &mut Plain).unwrap_err();
    assert_eq!(error, Refusal::Unsupported("label".into()));
    let cleared = with(vec![
        set_label(1, base(FLAGS, 0), text("a", None)),
        clear_label(2, base(FLAGS, 0)),
    ]);
    replay(&cleared, &limits, &mut Plain).unwrap();
}

#[test]
fn the_builtin_table_names_each_traced_flag_type() {
    let names: Vec<_> = (0..=18)
        .map(|t| builtin_label(t).map(|l| (l.text, l.number)))
        .collect();
    let some = |s: &str, n: Option<i32>| Some((s.to_string(), n));
    assert_eq!(names[0], some("ctf_flag_spawn", None));
    assert_eq!(names[1], some("ctf_flag_return", None));
    assert_eq!(names[2], some("assault_bomb_spawn", None));
    assert_eq!(names[3], some("assault_bomb_return", None));
    assert_eq!(names[4], some("oddball_spawn", None));
    assert_eq!(names[5], None);
    assert_eq!(names[6], None);
    assert_eq!(names[7], some("teleporter_sender", None));
    assert_eq!(names[8], some("teleporter_receiver", None));
    assert_eq!(names[9], None);
    assert_eq!(names[10], some("territory", None));
    for hill in 0..8u16 {
        assert_eq!(
            names[usize::from(11 + hill)],
            some("hill_corner", Some(i32::from(hill)))
        );
    }
    assert_eq!(builtin_label(19), None);
    assert_eq!(builtin_label(u16::MAX), None);
    // Every built-in label is one a document could set.
    for t in 0..=u16::MAX {
        if let Some(label) = builtin_label(t) {
            label.check().unwrap();
        }
    }
    // The flag types the link code names agree with the table.
    assert_eq!(
        builtin_label(TELEPORTER_SENDER).unwrap().text,
        "teleporter_sender"
    );
    assert_eq!(
        builtin_label(TELEPORTER_RECEIVER).unwrap().text,
        "teleporter_receiver"
    );
}
