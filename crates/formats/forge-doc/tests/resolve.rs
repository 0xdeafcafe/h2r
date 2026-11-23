//! Entry ops lowered to placement ops by running index
//! (docs/game/forge-map-variants.md, slice M2), against hand replays and
//! the editor on its invented cache.
mod support;

use std::collections::BTreeMap;

use forge_doc::{
    Before, BlockKind, Edit, Endpoint, Entry, EntryOp, F32, FieldWrite, ForgeDoc, Limits, LogHead,
    Op, OpId, Placement, Print, Proposal, Refusal, Value, fold_from, inverse, print, replay,
    resolve,
};
use scenario_edit::{Command, Session, Value as ScenarioValue};
use support::{ScenarioApply, base_counts, open, output};

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const WEAPONS: BlockKind = BlockKind::Weapons;
const STARTS: BlockKind = BlockKind::StartingLocations;

fn f(x: f32) -> F32 {
    F32::new(x).unwrap()
}

/// Flag `index` of an invented base, named with a print no map gives:
/// `resolve` reads identities, and the print is the adapter's check.
fn flag(index: u32) -> Endpoint {
    Endpoint::Base {
        block: FLAGS,
        index,
        print: Some(Print([index as u8; 8])),
    }
}

fn team(n: i64) -> FieldWrite {
    FieldWrite {
        field: "team".into(),
        value: Value::Int(n),
    }
}

fn set(target: Endpoint, n: i64) -> Edit {
    Edit::Entry(EntryOp::Set {
        target,
        write: team(n),
    })
}

fn remove(target: Endpoint) -> Edit {
    Edit::Entry(EntryOp::Remove { target })
}

fn reset(target: Endpoint) -> Edit {
    Edit::Entry(EntryOp::Reset { target })
}

fn add(block: BlockKind) -> Edit {
    Edit::Placement(Placement::Add {
        block,
        like: None,
        fields: vec![],
    })
}

fn ops(edits: Vec<Edit>) -> Vec<Op> {
    edits
        .into_iter()
        .enumerate()
        .map(|(i, edit)| Op {
            id: OpId(i as u32 + 1),
            edit,
        })
        .collect()
}

fn lowered_set(index: u32, n: i64) -> Placement {
    Placement::Set {
        block: FLAGS,
        index,
        write: team(n),
    }
}

fn lowered_remove(index: u32) -> Placement {
    Placement::Remove {
        block: FLAGS,
        index,
    }
}

#[test]
fn identities_lower_to_the_running_indices_of_a_hand_replay() {
    // Three base flags, b0 b1 b2. Each case: the ops, the placement op each
    // op ID lowers to, and each entry's final index.
    let added = Endpoint::Added(OpId(1));
    let cases: Vec<(
        &str,
        Vec<Edit>,
        Vec<(u32, Placement)>,
        Vec<(Endpoint, Option<u32>)>,
    )> = vec![
        (
            // [b0 b1 b2] -> remove b0 -> [b2 b1]
            "a removal swaps the last entry into the slot",
            vec![remove(flag(0)), set(flag(2), 1)],
            vec![(1, lowered_remove(0)), (2, lowered_set(0, 1))],
            vec![(flag(0), None), (flag(1), Some(1)), (flag(2), Some(0))],
        ),
        (
            // [b0 b1 b2 A] -> remove b0 -> [A b1 b2]
            "an added entry is followed through a removal",
            vec![add(FLAGS), remove(flag(0)), set(added, 4), set(flag(2), 5)],
            vec![
                (2, lowered_remove(0)),
                (3, lowered_set(0, 4)),
                (4, lowered_set(2, 5)),
            ],
            vec![(added, Some(0)), (flag(1), Some(1)), (flag(2), Some(2))],
        ),
        (
            // [b0 b1 b2] -> running remove 1 -> [b0 b2] -> remove b2 -> [b0]
            "placement ops by running index move identities too",
            vec![
                Edit::Placement(Placement::Remove {
                    block: FLAGS,
                    index: 1,
                }),
                set(flag(2), 6),
                remove(flag(2)),
            ],
            vec![(2, lowered_set(1, 6)), (3, lowered_remove(1))],
            vec![(flag(0), Some(0)), (flag(1), None), (flag(2), None)],
        ),
        (
            // The reset covers the removal, so b1 keeps its slot.
            "a reset leaves out the ops it covers and itself",
            vec![remove(flag(1)), set(flag(2), 7), reset(flag(1))],
            vec![(2, lowered_set(2, 7))],
            vec![(flag(1), Some(1)), (flag(2), Some(2))],
        ),
        (
            "an op after a reset lowers",
            vec![set(flag(0), 1), reset(flag(0)), set(flag(0), 2)],
            vec![(3, lowered_set(0, 2))],
            vec![(flag(0), Some(0))],
        ),
        (
            // [b0 b1 b2] -> remove b2 -> [b0 b1] -> remove b0 -> [b1]
            "removing the last entry moves nothing",
            vec![remove(flag(2)), remove(flag(0)), set(flag(1), 3)],
            vec![
                (1, lowered_remove(2)),
                (2, lowered_remove(0)),
                (3, lowered_set(0, 3)),
            ],
            vec![(flag(0), None), (flag(1), Some(0)), (flag(2), None)],
        ),
        (
            // Weapons are another block: their add moves no flag.
            "blocks are independent",
            vec![add(WEAPONS), remove(flag(0)), set(flag(1), 2)],
            vec![(2, lowered_remove(0)), (3, lowered_set(1, 2))],
            vec![(flag(1), Some(1)), (flag(2), Some(0))],
        ),
    ];
    let counts = BTreeMap::from([(FLAGS, 3), (WEAPONS, 2)]);
    for (name, edits, lowered, finals) in cases {
        let resolved = resolve(&ops(edits), &counts).unwrap();
        let got: Vec<_> = resolved
            .lowered
            .iter()
            .map(|(id, p)| (id.0, p.clone()))
            .collect();
        assert_eq!(got, lowered, "{name}");
        for (end, index) in finals {
            assert_eq!(resolved.index(end), index, "{name}: {end:?}");
        }
    }
}

#[test]
fn prints_are_collected_once_from_every_op_that_names_an_entry() {
    let counts = BTreeMap::from([(FLAGS, 3)]);
    let label = Edit::Forge(forge_doc::Forge::Label {
        target: flag(2),
        label: None,
    });
    let resolved = resolve(
        &ops(vec![
            set(flag(1), 1),
            set(flag(1), 2),
            label,
            set(flag(0), 3),
        ]),
        &counts,
    )
    .unwrap();
    let prints: Vec<_> = resolved.prints.iter().map(|p| (p.index, p.print)).collect();
    assert_eq!(
        prints,
        vec![(0, Print([0; 8])), (1, Print([1; 8])), (2, Print([2; 8]))]
    );

    let twice = ops(vec![
        set(flag(1), 1),
        set(
            Endpoint::Base {
                block: FLAGS,
                index: 1,
                print: Some(Print([9; 8])),
            },
            2,
        ),
    ]);
    assert!(resolve(&twice, &counts).is_err());
}

#[test]
fn a_block_an_entry_op_names_needs_its_base_count() {
    let error = resolve(&ops(vec![set(flag(0), 1)]), &BTreeMap::new()).unwrap_err();
    assert!(error.to_string().contains("count is needed"), "{error}");
    // Placement ops alone need no count.
    resolve(&ops(vec![add(FLAGS)]), &BTreeMap::new()).unwrap();
}

/// The fixture's entry `index` of `block`, named with its print.
fn own(target: &ScenarioApply, block: BlockKind, index: u32) -> Endpoint {
    Endpoint::Base {
        block,
        index,
        print: Some(print(block, &target.values(block, index as usize).unwrap()).unwrap()),
    }
}

fn write(field: &str, value: Value) -> FieldWrite {
    FieldWrite {
        field: field.into(),
        value,
    }
}

fn entry_set(target: Endpoint, field: &str, value: Value) -> Edit {
    Edit::Entry(EntryOp::Set {
        target,
        write: write(field, value),
    })
}

#[test]
fn lowered_entry_ops_match_the_editor_on_the_fixture() {
    let (session, source) = open();
    let (mut editor, _) = open();
    let mut target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: base_counts(&target.session),
        ..Limits::default()
    };
    let w0 = own(&target, WEAPONS, 0);
    let w1 = own(&target, WEAPONS, 1);
    let s0 = own(&target, STARTS, 0);
    let s1 = own(&target, STARTS, 1);
    let flag = own(&target, FLAGS, 0);
    let added = Endpoint::Added(OpId(2));
    let doc = support::draft(
        &target.source,
        vec![
            entry_set(w1, "position", Value::Vec3([f(7.0), f(8.0), f(9.0)])),
            Edit::Placement(Placement::Add {
                block: WEAPONS,
                like: Some(0),
                fields: vec![write("position", Value::Vec3([f(3.0), f(4.0), f(5.0)]))],
            }),
            // [w0 w1 A] -> [A w1]
            Edit::Entry(EntryOp::Remove { target: w0 }),
            entry_set(added, "scale", Value::Float(f(2.0))),
            entry_set(w1, "rotation", Value::Vec3([f(0.5), f(0.0), f(0.0)])),
            entry_set(s1, "team", Value::Int(3)),
            // [s0 s1] -> [s1]
            Edit::Entry(EntryOp::Remove { target: s0 }),
            entry_set(s1, "facing", Value::Float(f(1.25))),
            entry_set(flag, "identifier", Value::Int(5)),
        ],
    );
    let doc = ForgeDoc::from_bytes(doc.canonical_json().unwrap().as_bytes(), &limits).unwrap();
    replay(&doc, &limits, &mut target).unwrap();

    fn set(
        editor: &mut Session,
        block: BlockKind,
        index: usize,
        field: &'static str,
        value: ScenarioValue,
    ) {
        editor
            .apply(Command::set(block, index, field, value))
            .unwrap();
    }
    set(
        &mut editor,
        WEAPONS,
        1,
        "position",
        ScenarioValue::Vec3([7.0, 8.0, 9.0]),
    );
    let bytes = editor
        .scenario()
        .unwrap()
        .new_entry(WEAPONS, Some(0), Some([3.0, 4.0, 5.0]))
        .unwrap();
    editor.apply(Command::add(WEAPONS, bytes)).unwrap();
    editor.apply(Command::remove(WEAPONS, 0)).unwrap();
    set(&mut editor, WEAPONS, 0, "scale", ScenarioValue::Float(2.0));
    set(
        &mut editor,
        WEAPONS,
        1,
        "rotation",
        ScenarioValue::Vec3([0.5, 0.0, 0.0]),
    );
    set(&mut editor, STARTS, 1, "team", ScenarioValue::Int(3));
    editor.apply(Command::remove(STARTS, 0)).unwrap();
    set(&mut editor, STARTS, 0, "facing", ScenarioValue::Float(1.25));
    set(&mut editor, FLAGS, 0, "identifier", ScenarioValue::Int(5));

    assert_eq!(
        target.session.scenario().unwrap().changes(),
        editor.scenario().unwrap().changes()
    );
    assert_eq!(
        output(&target.source, &target.session),
        output(&target.source, &editor)
    );
}

#[test]
fn a_print_the_map_does_not_give_is_refused_before_any_write() {
    let (session, source) = open();
    let mut target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: base_counts(&target.session),
        ..Limits::default()
    };
    let w0 = own(&target, WEAPONS, 0);
    // Weapon 1 named with weapon 0's print: its position differs.
    let wrong = Endpoint::Base {
        block: WEAPONS,
        index: 1,
        print: w0.print(),
    };
    let doc = support::draft(
        &target.source,
        vec![
            entry_set(w0, "scale", Value::Float(f(2.0))),
            entry_set(wrong, "scale", Value::Float(f(3.0))),
        ],
    );
    assert_eq!(
        replay(&doc, &limits, &mut target),
        Err(Refusal::Print {
            block: WEAPONS,
            index: 1
        })
    );
    assert!(!target.session.scenario().unwrap().is_modified());
}

#[test]
fn a_reset_leaves_the_entry_with_the_fixtures_bytes() {
    let (session, source) = open();
    let mut target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: base_counts(&target.session),
        ..Limits::default()
    };
    let w0 = own(&target, WEAPONS, 0);
    let w1 = own(&target, WEAPONS, 1);
    let doc = support::draft(
        &target.source,
        vec![
            entry_set(w1, "position", Value::Vec3([f(7.0), f(8.0), f(9.0)])),
            entry_set(w1, "scale", Value::Float(f(4.0))),
            // [w0 w1] -> [w1]: weapon 1 moves to slot 0.
            Edit::Entry(EntryOp::Remove { target: w0 }),
            Edit::Entry(EntryOp::Reset { target: w1 }),
        ],
    );
    replay(&doc, &limits, &mut target).unwrap();
    let block = target.session.scenario().unwrap().block(WEAPONS);
    assert_eq!(block.len(), 1);
    assert_eq!(block.entry(0), Some(block.source_entries()[1].as_slice()));

    // A reset of every edit gives the fixture back byte for byte.
    let (session, source) = open();
    let mut target = ScenarioApply { session, source };
    let doc = support::draft(
        &target.source,
        vec![
            entry_set(w1, "scale", Value::Float(f(4.0))),
            Edit::Entry(EntryOp::Remove { target: w1 }),
            Edit::Entry(EntryOp::Reset { target: w1 }),
        ],
    );
    replay(&doc, &limits, &mut target).unwrap();
    assert!(!target.session.scenario().unwrap().is_modified());
    assert_eq!(
        output(&target.source, &target.session),
        target.source.bytes.as_deref().unwrap()
    );
}

fn stamp(head: LogHead, op: Op, limits: &Limits) -> (Entry, LogHead) {
    let entry = head
        .stamp(
            head.tick + 1,
            Proposal {
                author: "Editor".into(),
                base_seq: head.seq,
                op,
                was: None,
            },
            None,
            limits,
        )
        .unwrap();
    let next = LogHead {
        seq: entry.seq,
        tick: entry.tick,
        hash: entry.hash().unwrap(),
    };
    (entry, next)
}

#[test]
fn a_fold_of_entry_ops_then_their_undos_gives_the_start() {
    let (session, source) = open();
    let target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: base_counts(&target.session),
        ..Limits::default()
    };
    let w0 = own(&target, WEAPONS, 0);
    let w1 = own(&target, WEAPONS, 1);
    let start = support::draft(&target.source, vec![]);
    let genesis = LogHead::genesis(&start).unwrap();
    let edits = ops(vec![
        entry_set(w1, "scale", Value::Float(f(4.0))),
        Edit::Entry(EntryOp::Remove { target: w0 }),
        Edit::Entry(EntryOp::Reset { target: w1 }),
    ]);

    let mut head = genesis;
    let mut log = Vec::new();
    for op in &edits {
        let (entry, next) = stamp(head, op.clone(), &limits);
        log.push(entry);
        head = next;
    }
    let (folded, _) = fold_from(&start, genesis, &log, &limits).unwrap();
    assert_eq!(folded.ops, edits);
    assert_eq!(folded.format, forge_doc::FORMAT_ENTRIES);

    let (session, source) = open();
    let mut edited = ScenarioApply { session, source };
    replay(&folded, &limits, &mut edited).unwrap();
    assert!(edited.session.scenario().unwrap().is_modified());

    for op in edits.iter().rev() {
        let undo = inverse(op, &Before::capture(&start)).unwrap();
        let (entry, next) = stamp(head, undo, &limits);
        log.push(entry);
        head = next;
    }
    let (undone, _) = fold_from(&start, genesis, &log, &limits).unwrap();
    assert_eq!(undone, start);

    let (session, source) = open();
    let mut replayed = ScenarioApply { session, source };
    replay(&undone, &limits, &mut replayed).unwrap();
    assert!(!replayed.session.scenario().unwrap().is_modified());
}
