use std::collections::BTreeMap;

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn base(index: u32) -> Endpoint {
    Endpoint::Base {
        block: FLAGS,
        index,
    }
}

fn machine(index: u32) -> Endpoint {
    Endpoint::Base {
        block: BlockKind::Machines,
        index,
    }
}

fn forge(id: u32, edit: Forge) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(edit),
    }
}

fn teleporter(id: u32, sender: Endpoint, receiver: Endpoint, channel: u16) -> Op {
    forge(
        id,
        Forge::Link(Link::Teleporter {
            sender,
            receiver,
            channel,
        }),
    )
}

fn device(id: u32, from: Endpoint, to: Endpoint) -> Op {
    forge(id, Forge::Link(Link::Device { from, to }))
}

fn unlink(id: u32, link: u32) -> Op {
    forge(id, Forge::Unlink { link: OpId(link) })
}

fn remove(id: u32, block: BlockKind, index: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Remove { block, index }),
    }
}

fn counts(flags: u32, machines: u32) -> Limits {
    Limits {
        base_counts: BTreeMap::from([(FLAGS, flags), (BlockKind::Machines, machines)]),
        ..Limits::default()
    }
}

fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
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
fn link_ops_have_one_canonical_spelling() {
    let op = teleporter(3, base(0), Endpoint::Added(OpId(2)), 4);
    assert_eq!(
        crate::wire::canonical(&op, "op", MAX_DOC_BYTES).unwrap(),
        r#"{"edit":{"forge":{"link":{"teleporter":{"channel":4,"receiver":{"added":2},"sender":{"base":{"block":"netgame_flags","index":0}}}}}},"id":3}"#
    );
    assert_eq!(
        crate::wire::canonical(&unlink(4, 3), "op", MAX_DOC_BYTES).unwrap(),
        r#"{"edit":{"forge":{"unlink":{"link":3}}},"id":4}"#
    );
    let doc = with(vec![add(2, FLAGS), op, unlink(4, 3)]);
    let json = doc.canonical_json().unwrap();
    assert_eq!(
        ForgeDoc::from_bytes(json.as_bytes(), &Limits::default()).unwrap(),
        doc
    );
    // A device link lists its lower endpoint first, so the reverse spelling
    // of the same link is refused rather than hashed differently.
    validate(
        &with(vec![device(1, machine(0), machine(1))]),
        &counts(0, 2),
    )
    .unwrap();
    refuse(
        vec![device(1, machine(1), machine(0))],
        &counts(0, 2),
        "lower endpoint first",
    );
}

#[test]
fn every_link_rule_has_a_refusing_case() {
    let limits = counts(2, 2);
    let cases: Vec<(Vec<Op>, &str)> = vec![
        (vec![teleporter(1, base(0), base(0), 1)], "link to itself"),
        (
            vec![teleporter(1, base(0), base(1), MAX_CHANNEL + 1)],
            "identifier range",
        ),
        (
            vec![teleporter(1, machine(0), base(1), 1)],
            "want netgame_flags entries",
        ),
        (vec![device(1, base(0), base(1))], "want machines entries"),
        (
            vec![device(1, machine(0), machine(0))],
            "lower endpoint first",
        ),
        (
            vec![teleporter(1, base(0), base(2), 1)],
            "names no netgame_flags entry",
        ),
        (
            vec![teleporter(1, base(0), Endpoint::Added(OpId(9)), 1)],
            "names no netgame_flags entry",
        ),
        (
            vec![
                add(1, BlockKind::Weapons),
                teleporter(2, base(0), Endpoint::Added(OpId(1)), 1),
            ],
            "names no netgame_flags entry",
        ),
        (
            vec![teleporter(1, base(0), Endpoint::Added(OpId(0)), 1)],
            "want the ID of an add op",
        ),
        (
            vec![
                teleporter(1, base(0), base(1), 1),
                teleporter(2, base(0), base(1), 1),
            ],
            "already exists",
        ),
        (
            vec![
                teleporter(1, base(0), base(1), 1),
                teleporter(2, base(1), base(0), 1),
            ],
            "either a sender or a receiver",
        ),
        (
            vec![
                add(1, FLAGS),
                teleporter(2, base(0), base(1), 1),
                teleporter(3, base(0), Endpoint::Added(OpId(1)), 2),
            ],
            "already on channel 1",
        ),
        (vec![unlink(1, 0)], "want the ID of a link op"),
        (vec![unlink(1, 7)], "names no active link"),
        (vec![add(1, FLAGS), unlink(2, 1)], "names no active link"),
        (
            vec![
                teleporter(1, base(0), base(1), 1),
                unlink(2, 1),
                unlink(3, 1),
            ],
            "names no active link",
        ),
        (
            vec![teleporter(1, base(0), base(1), 1), remove(2, FLAGS, 1)],
            "remove the entry's links first",
        ),
        (
            vec![remove(1, FLAGS, 1), teleporter(2, base(0), base(1), 1)],
            "names no netgame_flags entry",
        ),
    ];
    for (ops, expected) in cases {
        refuse(ops, &limits, expected);
    }
}

#[test]
fn identity_follows_appends_and_swap_removes() {
    // Base flags 0 and 1. Flags 3 and 4 are added at indices 2 and 3.
    let ops = vec![
        add(3, FLAGS),
        add(4, FLAGS),
        teleporter(5, Endpoint::Added(OpId(3)), Endpoint::Added(OpId(4)), 6),
        // Removing base flag 0 moves the last flag (op 4) into index 0.
        remove(6, FLAGS, 0),
        teleporter(7, base(1), Endpoint::Added(OpId(4)), 6),
    ];
    let doc = with(ops);
    let limits = counts(2, 0);
    validate(&doc, &limits).unwrap();
    let links = doc.links(&limits.base_counts).unwrap();
    assert_eq!(
        links
            .active
            .iter()
            .map(|a| (a.id, a.indices))
            .collect::<Vec<_>>(),
        [(OpId(5), [2, 0]), (OpId(7), [1, 0])]
    );
    assert_eq!(
        links.teleporters,
        [
            TeleporterFlag {
                index: 0,
                sender: false,
                channel: 6
            },
            TeleporterFlag {
                index: 1,
                sender: true,
                channel: 6
            },
            TeleporterFlag {
                index: 2,
                sender: true,
                channel: 6
            },
        ]
    );
}

#[test]
fn unlink_leaves_no_resolved_link() {
    let limits = counts(2, 0);
    let linked = with(vec![teleporter(1, base(0), base(1), 3)]);
    assert_eq!(
        linked.links(&limits.base_counts).unwrap().teleporters.len(),
        2
    );
    let unlinked = with(vec![teleporter(1, base(0), base(1), 3), unlink(2, 1)]);
    assert_eq!(
        unlinked.links(&limits.base_counts).unwrap(),
        Links::default()
    );
    // A flag freed by the unlink can join another channel.
    let mut relinked = unlinked.ops.clone();
    relinked.push(teleporter(3, base(0), base(1), 9));
    validate(&with(relinked), &limits).unwrap();
}

#[test]
fn device_links_form_one_group_per_component() {
    let ops = vec![
        device(1, machine(3), machine(4)),
        device(2, machine(1), machine(2)),
        device(3, machine(0), machine(1)),
    ];
    let limits = counts(0, 5);
    let links = with(ops).links(&limits.base_counts).unwrap();
    assert_eq!(links.device_groups, [vec![0, 1, 2], vec![3, 4]]);
}

#[test]
fn links_need_the_base_count_only_to_resolve() {
    let doc = with(vec![teleporter(1, base(0), base(1), 3)]);
    validate(&doc, &Limits::default()).unwrap();
    let error = doc.links(&BTreeMap::new()).unwrap_err();
    assert!(error.to_string().contains("count is needed"), "{error}");
}

#[test]
fn links_fold_and_invert_like_other_edits() {
    let doc = draft();
    let link = teleporter(1, base(0), base(1), 2);
    let (entries, after) = log_entries(&doc, &[link.clone(), unlink(2, 1)]);
    assert_eq!(after.ops, [link.clone(), unlink(2, 1)]);
    // The link cannot be retracted while its unlink is the latest edit.
    let undo = inverse(&link, &Before::capture(&after)).unwrap();
    assert_eq!(undo.edit, Edit::Undo);
    let last = entries.last().unwrap();
    let at = LogHead {
        seq: last.seq,
        tick: last.tick,
        hash: last.hash().unwrap(),
    };
    let mut log = entries.clone();
    let refused = at
        .stamp(
            9,
            Proposal {
                author: "Editor".into(),
                base_seq: at.seq,
                op: undo,
                was: None,
            },
            None,
            &Limits::default(),
        )
        .unwrap();
    log.push(refused);
    assert_eq!(fold(&doc, &log), Err(Refusal::UndoOrder(OpId(1))));
}

#[test]
fn an_adapter_without_links_refuses_them_by_name() {
    struct Placements;
    impl Apply for Placements {
        fn placement(&mut self, _: &Placement) -> Result<()> {
            Ok(())
        }
        fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
            Ok(())
        }
    }
    let limits = counts(2, 0);
    let doc = with(vec![teleporter(1, base(0), base(1), 2)]);
    assert_eq!(
        replay(&doc, &limits, &mut Placements),
        Err(Refusal::Unsupported("link".into()))
    );
    let doc = with(vec![teleporter(1, base(0), base(1), 2), unlink(2, 1)]);
    replay(&doc, &limits, &mut Placements).unwrap();
}

fn log_entries(doc: &ForgeDoc, ops: &[Op]) -> (Vec<Entry>, ForgeDoc) {
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
    let after = fold(doc, &entries).unwrap();
    (entries, after)
}
