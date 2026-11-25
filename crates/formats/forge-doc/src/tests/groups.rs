use std::collections::BTreeMap;

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const WEAPONS: BlockKind = BlockKind::Weapons;

fn base(block: BlockKind, index: u32) -> Endpoint {
    Endpoint::base(block, index)
}

fn group_of(id: u32, name: &str, members: Vec<Endpoint>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Group(Group {
            name: name.into(),
            members,
            pivot_first: false,
            locked: false,
        })),
    }
}

fn ungroup(id: u32, group: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Ungroup { group: OpId(group) }),
    }
}

fn lock(id: u32, group: u32, locked: bool) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::GroupLock {
            group: OpId(group),
            locked,
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
fn a_group_op_makes_the_document_format_4_and_others_keep_their_number() {
    let doc = with(vec![group_of(
        1,
        "Base",
        vec![base(WEAPONS, 0), base(WEAPONS, 1)],
    )]);
    assert_eq!(doc.format, FORMAT_GROUPS);
    assert_eq!(FORMAT_GROUPS, 4);
    validate(&doc, &counts(0, 2)).unwrap();
    assert_eq!(with(vec![add(1, WEAPONS)]).format, FORMAT);
    let mut plain = with(vec![add(1, WEAPONS)]);
    plain.format = FORMAT_GROUPS;
    assert_eq!(
        validate(&plain, &Limits::default()),
        Err(Refusal::Format(FORMAT_GROUPS))
    );
}

#[test]
fn a_group_round_trips_through_canonical_json() {
    let mut doc = with(vec![
        add(1, WEAPONS),
        group_of(2, "Roof", vec![base(WEAPONS, 0), Endpoint::Added(OpId(1))]),
        lock(3, 2, true),
    ]);
    if let Edit::Forge(Forge::Group(g)) = &mut doc.ops[1].edit {
        g.pivot_first = true;
    }
    let limits = counts(0, 1);
    validate(&doc, &limits).unwrap();
    let json = doc.canonical_json().unwrap();
    assert_eq!(ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap(), doc);
}

#[test]
fn groups_resolve_to_final_indices_after_a_swap_remove() {
    // Base weapons 0..=2; weapon 0 is removed, so weapon 2 takes index 0.
    let doc = with(vec![
        group_of(1, "Pair", vec![base(WEAPONS, 2), base(WEAPONS, 1)]),
        lock(2, 1, true),
        remove(3, WEAPONS, 0),
    ]);
    let limits = counts(0, 3);
    validate(&doc, &limits).unwrap();
    let set = doc.groups(&limits.base_counts).unwrap();
    assert_eq!(set.active.len(), 1);
    let g = &set.active[0];
    assert_eq!((g.id, g.name.as_str(), g.locked), (OpId(1), "Pair", true));
    assert_eq!(g.members, vec![(WEAPONS, 0), (WEAPONS, 1)]);
}

#[test]
fn an_ungrouped_group_is_gone_and_its_members_are_free() {
    let ops = vec![
        group_of(1, "A", vec![base(WEAPONS, 0), base(WEAPONS, 1)]),
        ungroup(2, 1),
        group_of(3, "B", vec![base(WEAPONS, 1), base(WEAPONS, 2)]),
    ];
    let limits = counts(0, 3);
    let doc = with(ops);
    validate(&doc, &limits).unwrap();
    let set = doc.groups(&limits.base_counts).unwrap();
    assert_eq!(set.active.len(), 1);
    assert_eq!(set.active[0].name, "B");
}

#[test]
fn a_group_is_refused_by_name() {
    let l = counts(0, 3);
    let pair = vec![base(WEAPONS, 0), base(WEAPONS, 1)];
    refuse(
        vec![group_of(1, "A", vec![base(WEAPONS, 0)])],
        &l,
        "2 to 256 members",
    );
    refuse(
        vec![group_of(1, "A", vec![base(WEAPONS, 0), base(WEAPONS, 0)])],
        &l,
        "named twice",
    );
    refuse(vec![group_of(1, "", pair.clone())], &l, "must have a name");
    refuse(
        vec![group_of(1, &"x".repeat(33), pair.clone())],
        &l,
        "at most 32",
    );
    refuse(
        vec![group_of(1, "A\n", pair.clone())],
        &l,
        "control character",
    );
    refuse(
        vec![group_of(1, "A", vec![base(WEAPONS, 0), base(WEAPONS, 3)])],
        &l,
        "names no entry",
    );
    refuse(
        vec![
            group_of(1, "A", pair.clone()),
            group_of(2, "B", pair.clone()),
        ],
        &l,
        "already in a group",
    );
    refuse(vec![ungroup(1, 9)], &l, "names no active group");
    refuse(vec![lock(1, 9, true)], &l, "names no active group");
    refuse(vec![ungroup(1, 0)], &l, "ID of a group op");
}

#[test]
fn a_grouped_entry_cannot_be_removed_until_its_group_ends() {
    let l = counts(0, 3);
    let pair = vec![base(WEAPONS, 0), base(WEAPONS, 1)];
    refuse(
        vec![group_of(1, "A", pair.clone()), remove(2, WEAPONS, 0)],
        &l,
        "ungroup the entry first",
    );
    validate(
        &with(vec![
            group_of(1, "A", pair),
            ungroup(2, 1),
            remove(3, WEAPONS, 0),
        ]),
        &l,
    )
    .unwrap();
}

#[test]
fn an_adapter_that_keeps_no_groups_refuses_a_document_with_one() {
    struct Bare;
    impl Apply for Bare {
        fn placement(&mut self, _: &Placement) -> Result<()> {
            Ok(())
        }
        fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
            Ok(())
        }
    }
    let l = counts(0, 2);
    let doc = with(vec![group_of(
        1,
        "A",
        vec![base(WEAPONS, 0), base(WEAPONS, 1)],
    )]);
    assert_eq!(
        replay(&doc, &l, &mut Bare),
        Err(Refusal::Unsupported("group".into()))
    );
    // Group ops are not links: `links` is not asked.
    let ended = with(vec![
        group_of(1, "A", vec![base(WEAPONS, 0), base(WEAPONS, 1)]),
        ungroup(2, 1),
    ]);
    replay(&ended, &l, &mut Bare).unwrap();
}

#[test]
fn a_group_is_a_variant_tier_edit() {
    let doc = with(vec![group_of(
        1,
        "A",
        vec![base(WEAPONS, 0), base(WEAPONS, 1)],
    )]);
    assert_eq!(doc.tier(), Tier::Variant);
}
