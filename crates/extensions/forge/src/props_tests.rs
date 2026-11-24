//! `forge::props` and the session's field edits, removals and resets over
//! the fake world and invented map of `base_tests`.
use forge_doc::{Edit, Endpoint, EntryOp, FieldWrite, Forge, TagKey, Value};

use crate::base_tests::{Call, Fake, entry_ops, float, frame, open, rows, vec3, weapon};
use crate::{BlockKind, Frame, Linkable, Teleporter, ThingKind, props};

/// Another value of the same type.
fn other(value: &Value) -> Value {
    match value {
        Value::Int(n) => Value::Int(n + 1),
        Value::Float(f) => float(f.get() + 0.5),
        Value::Vec3(v) => vec3([v[0].get() + 1., v[1].get(), v[2].get()]),
        Value::Tag(Some(key)) => Value::Tag(Some(TagKey {
            group: key.group.clone(),
            path: format!("{}2", key.path),
        })),
        value => panic!("no other value for {value:?}"),
    }
}

#[test]
fn the_table_gives_each_kind_its_rows_and_the_field_each_writes() {
    let rows = |kind, block| {
        props::of_kind(kind)
            .map(|p| (p.name, p.field(block).map(|f| f.name)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(ThingKind::Object, BlockKind::Weapons),
        [
            ("position", Some("position")),
            ("rotation", Some("rotation")),
            ("object", Some("palette index")),
        ]
    );
    assert_eq!(
        rows(ThingKind::ItemSpawner, BlockKind::NetgameEquipment),
        [
            ("position", Some("position")),
            ("rotation", Some("orientation")),
            ("item", Some("item")),
            ("respawn time", Some("respawn time")),
            ("return time", Some("abandon limit")),
        ]
    );
    assert_eq!(
        rows(ThingKind::StartingPoint, BlockKind::StartingLocations),
        [
            ("position", Some("position")),
            ("rotation", Some("facing")),
            ("team", Some("team")),
        ]
    );
    assert_eq!(
        rows(ThingKind::Goal, BlockKind::NetgameFlags),
        [
            ("position", Some("position")),
            ("rotation", Some("facing")),
            ("team", Some("team")),
            ("role", Some("type")),
            ("order", Some("identifier")),
        ]
    );
    // A teleporter's channel is its link's, not a row.
    assert_eq!(
        rows(ThingKind::Teleporter, BlockKind::NetgameFlags),
        [("position", Some("position")), ("rotation", Some("facing"))]
    );
}

#[test]
fn each_rows_write_is_one_entry_set_and_one_base_call() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let mut tick = 0;
    let mut written = 0;
    for thing in s.things() {
        let Endpoint::Base { block, index, .. } = thing.id else {
            panic!("only own things before any add")
        };
        for prop in props::of(&thing).collect::<Vec<_>>() {
            let value = other(prop.read(&thing).unwrap());
            let field = prop.field(block).unwrap().name;
            let calls = w.calls.len();
            tick += 1;
            s.set_field(&mut w, tick, thing.id, prop.name, value.clone())
                .unwrap();
            assert_eq!(
                entry_ops(&s),
                [EntryOp::Set {
                    target: thing.id,
                    write: FieldWrite {
                        field: field.into(),
                        value: value.clone(),
                    },
                }],
                "{} {}",
                thing.kind.name(),
                prop.name
            );
            let made = &w.calls[calls..];
            match prop.name {
                "position" | "rotation" => {
                    assert!(
                        matches!(made, [Call::BaseMove(b, i, _)] if (*b, *i) == (block, index)),
                        "{made:?}"
                    );
                }
                _ => assert_eq!(made, [Call::BaseField(block, index, field.into(), value)]),
            }
            tick += 1;
            s.undo(&mut w, tick).unwrap();
            written += 1;
        }
    }
    // In the things' order: two weapons with three rows each, a starting
    // point with three, a teleporter with two, a goal with five, a second
    // teleporter with two and a spawner with five.
    assert_eq!(written, 3 + 3 + 3 + 2 + 5 + 2 + 5);
}

#[test]
fn an_added_things_rows_write_entry_ops_by_its_add() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let id = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let thing = s.thing(Endpoint::Added(id)).unwrap();
    assert_eq!(
        props::of(&thing).map(|p| p.name).collect::<Vec<_>>(),
        ["position", "rotation", "object"]
    );
    s.set_field(
        &mut w,
        2,
        Endpoint::Added(id),
        "position",
        vec3([5., 1., 2.]),
    )
    .unwrap();
    assert_eq!(
        s.document().ops[1].edit,
        Edit::Entry(EntryOp::Set {
            target: Endpoint::Added(id),
            write: FieldWrite {
                field: "position".into(),
                value: vec3([5., 1., 2.]),
            },
        })
    );
    assert_eq!(
        w.calls[1..],
        [Call::Move(
            1,
            Frame {
                position: [5., 1., 2.],
                rotation: [0.; 3],
            }
        )]
    );
    // Setting what it already holds writes nothing.
    let object = thing.fields[0].1.clone();
    s.set_field(&mut w, 3, Endpoint::Added(id), "object", object)
        .unwrap();
    assert_eq!(s.document().ops.len(), 2);
}

#[test]
fn a_property_not_in_the_table_is_absent() {
    for name in ["spare clips", "weight", "game types", "channel", "scale"] {
        assert!(props::prop(name).is_none(), "{name}");
    }
    let mut s = open(rows());
    let mut w = Fake::default();
    let start = Endpoint::base(BlockKind::StartingLocations, 0);
    let thing = s.thing(start).unwrap();
    assert!(props::of(&thing).all(|p| p.name != "weight"));
    assert_eq!(
        s.set_field(&mut w, 1, start, "weight", Value::Int(2))
            .unwrap_err(),
        "This starting point has no weight."
    );
    // A row of another kind is absent too.
    assert_eq!(
        s.set_field(&mut w, 1, weapon(0), "team", Value::Int(1))
            .unwrap_err(),
        "This object has no team."
    );
    let flag = Endpoint::base(BlockKind::NetgameFlags, 0);
    assert_eq!(
        s.set_field(&mut w, 1, flag, "order", Value::Int(4))
            .unwrap_err(),
        "This teleporter has no order."
    );
    assert!(s.document().ops.is_empty());
    assert!(w.calls.is_empty());
}

#[test]
fn remove_makes_the_row_inert_and_undo_writes_the_kept_row_back() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let print = rows()[1].print;
    s.remove(&mut w, 1, weapon(1)).unwrap();
    assert_eq!(
        entry_ops(&s),
        [EntryOp::Remove {
            target: Endpoint::Base {
                block: BlockKind::Weapons,
                index: 1,
                print: Some(print),
            },
        }]
    );
    assert_eq!(w.calls, [Call::BaseInert(BlockKind::Weapons, 1)]);
    assert!(s.thing(weapon(1)).is_none());
    assert_eq!(
        s.things()
            .iter()
            .filter(|t| t.block == BlockKind::Weapons)
            .count(),
        1
    );
    s.undo(&mut w, 2).unwrap();
    assert_eq!(
        w.calls[1..],
        [Call::BaseRestore(
            BlockKind::Weapons,
            1,
            rows()[1].raw.clone()
        )]
    );
    assert!(s.thing(weapon(1)).is_some());

    // A moved entry comes back as the disc has it, then where it was moved.
    let to = frame(2., 0.5);
    s.move_thing(&mut w, 3, weapon(1), to).unwrap();
    s.remove(&mut w, 4, weapon(1)).unwrap();
    s.undo(&mut w, 5).unwrap();
    assert_eq!(
        w.calls[2..],
        [
            Call::BaseMove(BlockKind::Weapons, 1, to),
            Call::BaseInert(BlockKind::Weapons, 1),
            Call::BaseRestore(BlockKind::Weapons, 1, rows()[1].raw.clone()),
            Call::BaseMove(BlockKind::Weapons, 1, to),
        ]
    );
}

#[test]
fn removing_a_linked_entry_ends_its_links_in_the_same_step() {
    let mut s = open(rows())
        .with_teleporters(vec![
            Teleporter {
                index: 0,
                position: [0., 5., 0.],
                sender: true,
                channel: 1,
            },
            Teleporter {
                index: 2,
                position: [2., 5., 0.],
                sender: false,
                channel: 3,
            },
        ])
        .unwrap();
    let mut w = Fake::default();
    let link = s
        .link(&mut w, 1, Linkable::Teleporter(0), Linkable::Teleporter(2))
        .unwrap();
    assert_eq!(w.calls, [Call::Channel(2, 1)]);

    s.remove(&mut w, 2, Endpoint::base(BlockKind::NetgameFlags, 0))
        .unwrap();
    let ops = &s.document().ops;
    assert_eq!(ops.len(), 3);
    assert_eq!(ops[1].edit, Edit::Forge(Forge::Unlink { link }));
    assert!(matches!(ops[2].edit, Edit::Entry(EntryOp::Remove { .. })));
    assert!(s.links_of(Linkable::Teleporter(2)).is_empty());
    // The receiver takes its own channel back; the removed flag is inert.
    assert_eq!(
        w.calls[1..],
        [
            Call::BaseInert(BlockKind::NetgameFlags, 0),
            Call::Channel(2, 3),
        ]
    );
    assert_eq!(
        s.teleporters().iter().map(|t| t.index).collect::<Vec<_>>(),
        [2]
    );

    // One undo brings back the flag and its link.
    s.undo(&mut w, 3).unwrap();
    assert_eq!(s.document().ops.len(), 1);
    assert_eq!(s.links_of(Linkable::Teleporter(0)).len(), 1);
    assert_eq!(
        w.calls[3..],
        [
            Call::BaseRestore(BlockKind::NetgameFlags, 0, rows()[3].raw.clone()),
            Call::Channel(2, 1),
        ]
    );
}

#[test]
fn reset_puts_an_own_entry_back_as_the_map_has_it() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let start = Endpoint::base(BlockKind::StartingLocations, 0);
    let first = s.thing(start).unwrap();
    s.move_thing(&mut w, 1, start, frame(2., 0.25)).unwrap();
    s.set_field(&mut w, 2, start, "team", Value::Int(1))
        .unwrap();
    assert!(s.thing(start).unwrap().changed);
    let calls = w.calls.len();

    s.reset(&mut w, 3, start).unwrap();
    assert_eq!(
        entry_ops(&s).last(),
        Some(&EntryOp::Reset { target: first.id })
    );
    assert_eq!(
        w.calls[calls..],
        [
            Call::BaseMove(BlockKind::StartingLocations, 0, first.frame),
            Call::BaseField(
                BlockKind::StartingLocations,
                0,
                "team".into(),
                Value::Int(0)
            ),
        ]
    );
    let back = s.thing(start).unwrap();
    assert_eq!((back.fields, back.changed), (first.fields, false));

    // Nothing is left to reset, and an added thing has no original.
    assert!(
        s.reset(&mut w, 4, start)
            .unwrap_err()
            .contains("already as the map has it")
    );
    let id = s.place(&mut w, 5, 0, frame(1., 0.)).unwrap();
    assert!(s.reset(&mut w, 6, Endpoint::Added(id)).is_err());

    // A removed entry comes back with its kept row.
    s.remove(&mut w, 7, weapon(0)).unwrap();
    s.reset(&mut w, 8, weapon(0)).unwrap();
    assert_eq!(
        w.calls.last(),
        Some(&Call::BaseRestore(
            BlockKind::Weapons,
            0,
            rows()[0].raw.clone()
        ))
    );
    assert!(s.thing(weapon(0)).is_some());
    // Undoing the reset removes it again.
    s.undo(&mut w, 9).unwrap();
    assert_eq!(
        w.calls.last(),
        Some(&Call::BaseInert(BlockKind::Weapons, 0))
    );
}

#[test]
fn game_types_are_the_slots_read_and_a_set_change_is_one_op_a_changed_slot() {
    use forge_doc::games::GameSet;
    let spawner = Endpoint::base(BlockKind::NetgameEquipment, 0);
    let mut s = open(rows());
    let mut w = Fake::default();
    let thing = s.thing(spawner).unwrap();
    // The map's spawner has the group value in every slot: every engine.
    assert_eq!(props::games(&thing), Some(GameSet::ALL));
    assert_eq!(props::games(&s.thing(weapon(0)).unwrap()), None);

    // Slayer and Oddball: [2, 3, 2, 2] changes all four slots.
    let set = GameSet::of(&[2, 3]);
    s.set_games(&mut w, 1, spawner, set).unwrap();
    let slot = |n: usize, v: i64| EntryOp::Set {
        target: spawner_target(),
        write: FieldWrite {
            field: props::GAME_SLOTS[n].into(),
            value: Value::Int(v),
        },
    };
    assert_eq!(
        entry_ops(&s),
        [slot(0, 2), slot(1, 3), slot(2, 2), slot(3, 2)]
    );
    assert_eq!(props::games(&s.thing(spawner).unwrap()), Some(set));
    // One undo step puts all four back.
    s.undo(&mut w, 2).unwrap();
    assert_eq!(props::games(&s.thing(spawner).unwrap()), Some(GameSet::ALL));

    // The same set writes nothing; a set that differs in one slot writes one.
    s.set_games(&mut w, 3, spawner, GameSet::ALL).unwrap();
    s.set_games(&mut w, 4, spawner, GameSet::of(&[2])).unwrap();
    let before = s.document().ops.len();
    s.set_games(&mut w, 5, spawner, GameSet::of(&[2])).unwrap();
    assert_eq!(s.document().ops.len(), before);
    s.set_games(&mut w, 6, spawner, GameSet::of(&[2, 3]))
        .unwrap();
    assert_eq!(
        s.document().ops.len(),
        before + 1,
        "only the second slot changes"
    );
}

fn spawner_target() -> Endpoint {
    Endpoint::Base {
        block: BlockKind::NetgameEquipment,
        index: 0,
        print: Some(rows().into_iter().last().unwrap().print),
    }
}

#[test]
fn a_set_the_slots_cannot_name_is_refused_with_its_sentence_and_nothing_is_written() {
    use forge_doc::games::GameSet;
    let spawner = Endpoint::base(BlockKind::NetgameEquipment, 0);
    let mut s = open(rows());
    let mut w = Fake::default();
    let too_many = GameSet::of(&[4, 2, 3, 7, 1, 9]);
    assert_eq!(
        s.set_games(&mut w, 1, spawner, too_many).unwrap_err(),
        "An item can name four game types at most."
    );
    assert_eq!(
        s.set_games(&mut w, 1, spawner, GameSet::of(&[1]))
            .unwrap_err(),
        "Capture the Flag and Assault go together."
    );
    assert_eq!(
        s.set_games(&mut w, 1, weapon(0), GameSet::ALL).unwrap_err(),
        "This object has no game types to edit."
    );
    assert!(s.document().ops.is_empty());
    assert!(w.calls.is_empty());
}

#[test]
fn a_spawners_team_is_a_property_op_that_touches_nothing_in_the_world() {
    let spawner = Endpoint::base(BlockKind::NetgameEquipment, 0);
    let mut s = open(rows());
    let mut w = Fake::default();
    assert_eq!(s.team(spawner), None);
    s.set_team(&mut w, 1, spawner, Some(3)).unwrap();
    assert_eq!(s.team(spawner), Some(3));
    assert_eq!(
        entry_ops(&s),
        [EntryOp::Property {
            target: spawner_target(),
            property: forge_doc::Property::Team(Some(3)),
        }]
    );
    assert!(w.calls.is_empty(), "no scenario bytes: {:?}", w.calls);
    // The same team writes nothing; a clear is one op; undo takes it back.
    s.set_team(&mut w, 2, spawner, Some(3)).unwrap();
    assert_eq!(s.document().ops.len(), 1);
    s.set_team(&mut w, 3, spawner, None).unwrap();
    assert_eq!((s.document().ops.len(), s.team(spawner)), (2, None));
    s.undo(&mut w, 4).unwrap();
    assert_eq!(s.team(spawner), Some(3));
    // Another kind has none, and a team past 7 is refused.
    assert_eq!(
        s.set_team(&mut w, 5, weapon(0), Some(1)).unwrap_err(),
        "This object has no team."
    );
    assert!(s.set_team(&mut w, 5, spawner, Some(8)).is_err());
    assert_eq!(s.document().ops.len(), 1);
}
