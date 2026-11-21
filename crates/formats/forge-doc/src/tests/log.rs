use super::*;

fn stamp(head: LogHead, op: Op, author: &str, tick: u32) -> Entry {
    let holds = match &op.edit {
        Edit::TagField(write) => write.was,
        _ => None,
    };
    head.stamp(
        tick,
        Proposal {
            author: author.into(),
            base_seq: head.seq,
            op,
            was: None,
        },
        holds,
        &Limits::default(),
    )
    .unwrap()
}

fn head(entry: &Entry) -> LogHead {
    LogHead {
        seq: entry.seq,
        tick: entry.tick,
        hash: entry.hash().unwrap(),
    }
}

fn apply_entries(doc: &ForgeDoc, originals: &[Op]) -> (Vec<Entry>, ForgeDoc) {
    let mut entries = Vec::new();
    let mut at = LogHead::genesis(doc).unwrap();
    for op in originals {
        let entry = stamp(at, op.clone(), "Editor", at.tick + 1);
        at = head(&entry);
        entries.push(entry);
    }
    let after = fold(doc, &entries).unwrap();
    (entries, after)
}

#[test]
fn host_entry_golden_vector() {
    let entry = stamp(LogHead::genesis(&draft()).unwrap(), scale(), "Editor", 90);
    assert_eq!(
        entry.canonical_json().unwrap(),
        r#"{"author":"Editor","op":{"edit":{"placement":{"set":{"block":"weapons","index":0,"write":{"field":"scale","value":{"float":"80000000"}}}}},"id":1},"prev_hash":"77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001","seq":1,"tick":90}"#
    );
    assert_eq!(
        entry.hash().unwrap().to_string(),
        "030c1227e8f49097f024c4d6fcceb4e3363d3cfa4ca1340f4c9003c341e20f0f"
    );
    assert_eq!(
        Entry::from_bytes(
            entry.canonical_json().unwrap().as_bytes(),
            &Limits::default()
        )
        .unwrap(),
        entry
    );
}

#[test]
fn fold_then_unfold_with_inverses() {
    let mut doc = draft();
    doc.expect = Some(Expected {
        map_id: MapId([3; 32]),
        geometry: Sha256([4; 32]),
    });
    doc.ops.push(set(
        20,
        BlockKind::StartingLocations,
        0,
        "team",
        Value::Int(2),
    ));
    let originals = [
        scale(),
        add(2, BlockKind::Weapons),
        Op {
            id: OpId(3),
            edit: Edit::Placement(Placement::Remove {
                block: BlockKind::Weapons,
                index: 0,
            }),
        },
        Op {
            id: OpId(4),
            edit: Edit::TagField(tag_write()),
        },
    ];

    let (mut log, after) = apply_entries(&doc, &originals);
    assert_eq!(&after.ops[1..], &originals[..]);
    assert_eq!(after.expect, None);

    let mut at = head(log.last().unwrap());
    for op in originals.iter().rev() {
        let edit = inverse(op, &Before::capture(&doc)).unwrap();
        let entry = stamp(at, edit, "Editor", at.tick);
        at = head(&entry);
        log.push(entry);
    }
    let restored = fold(&doc, &log).unwrap();
    assert_eq!(restored, doc);
    assert_eq!(restored.id().unwrap(), doc.id().unwrap());

    let undo = inverse(originals.last().unwrap(), &Before::capture(&doc)).unwrap();
    let redo = inverse(&undo, &Before::capture(&after)).unwrap();
    assert_eq!(&redo, originals.last().unwrap());
    log.truncate(originals.len() + 1);
    log.push(stamp(head(log.last().unwrap()), redo, "Editor", at.tick));
    assert_eq!(fold(&doc, &log).unwrap(), after);
}

#[test]
fn detects_log_tampering_truncation_and_bad_order() {
    let doc = draft();
    let start = LogHead::genesis(&doc).unwrap();
    let limits = Limits::default();
    let (entries, after) = apply_entries(&doc, &[scale(), add(2, BlockKind::Weapons)]);
    let expected = head(&entries[1]);
    assert_eq!(
        validate_chain(start, &entries, Some(expected), &limits).unwrap(),
        expected
    );
    assert_eq!(
        fold_from(
            &fold(&doc, &entries[..1]).unwrap(),
            head(&entries[0]),
            &entries[1..],
            &limits
        )
        .unwrap(),
        (after, expected)
    );
    let second = &entries[1];
    let mut variants = vec![vec![second.clone()], entries[..1].to_vec()];
    let mut reordered = entries.clone();
    reordered.swap(0, 1);
    variants.push(reordered);
    let mut changed = entries.clone();
    changed[0].author = "Another editor".into();
    variants.push(changed);
    let mut changed = entries.clone();
    changed[1].op = add(2, BlockKind::Vehicles);
    variants.push(changed);
    let mut changed = entries.clone();
    changed[1].tick = 0;
    variants.push(changed);
    let mut changed = entries.clone();
    changed[0].prev_hash = Sha256([9; 32]);
    variants.push(changed);
    for invalid in variants {
        assert!(matches!(
            validate_chain(start, &invalid, Some(expected), &limits),
            Err(Refusal::Chain { .. })
        ));
    }

    let forged_start = LogHead {
        hash: Sha256([3; 32]),
        ..start
    };
    assert!(matches!(
        fold_from(&doc, forged_start, &[], &limits),
        Err(Refusal::Chain { seq: 0, .. })
    ));
    let forged_tick = LogHead { tick: 1, ..start };
    assert!(validate_chain(forged_tick, &[], None, &limits).is_err());
}

#[test]
fn refuses_stale_or_mismatched_proposals() {
    let at = LogHead {
        tick: 12,
        seq: 7,
        hash: Sha256([0x77; 32]),
    };
    let limits = Limits::default();
    let holds = Value::Int(1).digest().unwrap();
    let proposal = Proposal {
        author: "Editor".into(),
        base_seq: 7,
        op: scale(),
        was: Some(holds),
    };
    let bytes = serde_json::to_vec(&proposal).unwrap();
    assert_eq!(Proposal::from_bytes(&bytes, &limits).unwrap(), proposal);
    assert_eq!(
        at.stamp(11, proposal.clone(), Some(holds), &limits)
            .unwrap_err(),
        Refusal::Chain {
            seq: 8,
            reason: "tick goes backwards".into()
        }
    );
    assert_eq!(
        at.stamp(12, proposal.clone(), None, &limits).unwrap_err(),
        Refusal::Mismatch
    );
    let mut stale = proposal.clone();
    stale.base_seq -= 1;
    assert_eq!(
        at.stamp(12, stale, Some(holds), &limits).unwrap_err(),
        Refusal::Stale {
            proposed: 6,
            head: 7
        }
    );
    let mut write = proposal.clone();
    write.op.edit = Edit::TagField(tag_write());
    assert_eq!(
        at.stamp(12, write, Some(holds), &limits).unwrap_err(),
        Refusal::Mismatch
    );
    let full = LogHead {
        seq: u32::MAX,
        ..at
    };
    let proposal = Proposal {
        base_seq: u32::MAX,
        ..proposal
    };
    assert!(matches!(
        full.stamp(12, proposal, Some(holds), &limits),
        Err(Refusal::Chain { .. })
    ));
}

#[test]
fn refuses_invalid_inverses_and_rebound_ids() {
    let doc = draft();
    let first = scale();
    let second = add(2, BlockKind::Weapons);
    let (mut log, after) = apply_entries(&doc, &[first.clone(), second.clone()]);
    let at = head(&log[1]);
    let first_undo = inverse(&first, &Before::capture(&doc)).unwrap();
    let last_undo = inverse(&second, &Before::capture(&doc)).unwrap();
    let try_entry = |op, author| {
        let mut invalid = log.clone();
        invalid.push(stamp(at, op, author, at.tick));
        fold(&doc, &invalid).unwrap_err()
    };
    assert_eq!(
        try_entry(first_undo.clone(), "Editor"),
        Refusal::UndoOrder(first.id)
    );
    assert_eq!(
        try_entry(last_undo.clone(), "Another editor"),
        Refusal::UndoAuthor(second.id)
    );
    assert_eq!(
        try_entry(second.clone(), "Editor"),
        Refusal::DuplicateOp(second.id)
    );
    assert!(inverse(&first_undo, &Before::capture(&after)).is_err());

    log.push(stamp(at, last_undo, "Editor", at.tick));
    log.push(stamp(
        head(log.last().unwrap()),
        add(2, BlockKind::Vehicles),
        "Editor",
        at.tick,
    ));
    assert_eq!(
        fold(&doc, &log).unwrap_err(),
        Refusal::DuplicateOp(second.id)
    );
}

#[test]
fn log_wire_and_batch_caps() {
    let start = LogHead::genesis(&draft()).unwrap();
    let entry = stamp(start, scale(), "Editor", 1);
    let bytes = vec![b' '; MAX_ENTRY_BYTES + 1];
    let limits = Limits {
        max_log_entries: 0,
        ..Limits::default()
    };
    assert_eq!(
        Entry::from_bytes(&bytes, &limits).unwrap_err(),
        Refusal::limit("entry bytes", MAX_ENTRY_BYTES)
    );
    assert_eq!(
        Proposal::from_bytes(&bytes, &limits).unwrap_err(),
        Refusal::limit("proposal bytes", MAX_ENTRY_BYTES)
    );
    assert_eq!(
        validate_chain(start, std::slice::from_ref(&entry), None, &limits).unwrap_err(),
        Refusal::limit("log entries", 0)
    );
}

#[test]
fn folding_an_inverse_restores_the_block_count_and_add_budget() {
    let doc = draft();
    let start = LogHead::genesis(&doc).unwrap();
    let limits = Limits {
        base_counts: [(BlockKind::Weapons, 127)].into(),
        max_adds: 1,
        ..Limits::default()
    };
    let first = stamp(start, add(1, BlockKind::Weapons), "Editor", 1);
    let next = add(2, BlockKind::Weapons);
    let overflow = stamp(head(&first), next.clone(), "Editor", 2);
    assert_eq!(
        fold_from(&doc, start, &[first.clone(), overflow], &limits).unwrap_err(),
        Refusal::limit("weapons", 128)
    );

    let undo = inverse(&first.op, &Before::capture(&doc)).unwrap();
    let undo = stamp(head(&first), undo, "Editor", 2);
    let last = stamp(head(&undo), next.clone(), "Editor", 3);
    let (folded, _) = fold_from(&doc, start, &[first, undo, last], &limits).unwrap();
    assert_eq!(folded.ops, vec![next]);
}
