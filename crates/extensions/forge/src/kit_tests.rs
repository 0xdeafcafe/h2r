//! Kit sessions over the fake world and invented map of `base_tests`: an
//! added starting point, spawner or teleporter raises its block's count in
//! the running game, an undo puts it back, and a map without the kit
//! refuses the add by name.
use std::collections::BTreeMap;

use forge_doc::{Edit, Endpoint, Placement, Tier, Value};
use map::scenario_layout as layout;

use crate::base_tests::{Call, Fake, float, frame, open, rows, tag, vec3};
use crate::{BlockKind, Goal, Session, ThingKind, kit};

/// A kit session: room for two more starting points and one more flag and
/// spawner.
fn kit_session() -> Session {
    open(rows())
        .with_kit(BTreeMap::from([
            (BlockKind::StartingLocations, 2),
            (BlockKind::NetgameFlags, 3),
            (BlockKind::NetgameEquipment, 1),
        ]))
        .unwrap()
}

fn fields(calls: &[(&str, Value)]) -> Vec<(String, Value)> {
    calls.iter().map(|(n, v)| (n.to_string(), v.clone())).collect()
}

fn rifles() -> forge_doc::TagKey {
    tag("itmc", r"fixture\rifles")
}

#[test]
fn an_added_spawn_writes_the_next_spare_row_and_raises_the_count() {
    let mut s = kit_session();
    let mut w = Fake::default();
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(2));
    let id = s.place_spawn(&mut w, 1, 1, frame(1., 0.5)).unwrap();
    // The map has one starting point, so the new row is its second.
    assert_eq!(
        w.calls,
        [Call::KitAdd(
            BlockKind::StartingLocations,
            1,
            fields(&[
                ("position", vec3([1., 1., 2.])),
                ("facing", float(0.5)),
                ("team", Value::Int(1)),
            ]),
        )]
    );
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(1));
    let thing = s.thing(Endpoint::Added(id)).unwrap();
    assert_eq!(thing.kind, ThingKind::StartingPoint);
    assert!(!thing.own && thing.changed);
    assert_eq!(thing.frame, frame(1., 0.5));
    assert!(matches!(
        &s.document().ops[0].edit,
        Edit::Placement(Placement::Add {
            block: BlockKind::StartingLocations,
            like: None,
            ..
        })
    ));
}

#[test]
fn undo_takes_the_row_back_and_restores_the_room() {
    let mut s = kit_session();
    let mut w = Fake::default();
    let start = s.document().clone();
    s.place_spawn(&mut w, 1, 0, frame(1., 0.)).unwrap();
    s.undo(&mut w, 2).unwrap();
    assert_eq!(
        w.calls.last(),
        Some(&Call::KitDrop(BlockKind::StartingLocations, 1))
    );
    assert_eq!(s.document().ops, start.ops);
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(2));
    assert_eq!(s.things().len(), 7);

    // A redo writes the same row again.
    s.redo(&mut w, 3).unwrap();
    assert_eq!(w.calls[0], w.calls[2]);
}

#[test]
fn a_move_edits_the_added_row_and_a_removal_drops_it() {
    let mut s = kit_session();
    let mut w = Fake::default();
    let id = s.place_spawn(&mut w, 1, 0, frame(1., 0.5)).unwrap();
    let thing = Endpoint::Added(id);
    s.move_thing(&mut w, 2, thing, frame(3., 1.0)).unwrap();
    assert_eq!(
        w.calls[1..],
        [Call::BaseMove(BlockKind::StartingLocations, 1, frame(3., 1.0))]
    );
    s.undo(&mut w, 3).unwrap();
    assert_eq!(
        w.calls[2],
        Call::BaseMove(BlockKind::StartingLocations, 1, frame(1., 0.5))
    );

    s.set_field(&mut w, 4, thing, "team", Value::Int(3)).unwrap();
    assert_eq!(
        w.calls[3],
        Call::BaseField(BlockKind::StartingLocations, 1, "team".into(), Value::Int(3))
    );

    s.remove(&mut w, 5, thing).unwrap();
    assert_eq!(w.calls[4], Call::KitDrop(BlockKind::StartingLocations, 1));
    assert!(s.thing(thing).is_none());
    s.undo(&mut w, 6).unwrap();
    assert_eq!(
        w.calls[5],
        Call::KitAdd(
            BlockKind::StartingLocations,
            1,
            fields(&[
                ("position", vec3([1., 1., 2.])),
                ("facing", float(0.5)),
                ("team", Value::Int(3)),
            ]),
        )
    );
}

#[test]
fn a_spawner_is_one_of_the_items_the_maps_spawners_name() {
    let mut s = kit_session();
    let mut w = Fake::default();
    assert_eq!(s.kit_items(), [rifles()]);
    assert_eq!(kit::item_group(&rifles()), kit::ItemGroup::Weapons);
    s.place_item(&mut w, 1, &rifles(), frame(2., 0.)).unwrap();
    // Every game type, then where it is and what it is.
    assert_eq!(
        w.calls,
        [Call::KitAdd(
            BlockKind::NetgameEquipment,
            1,
            fields(&[
                ("game type 1", Value::Int(i64::from(layout::game_types::ALL))),
                ("game type 2", Value::Int(i64::from(layout::game_types::ALL))),
                ("game type 3", Value::Int(i64::from(layout::game_types::ALL))),
                ("game type 4", Value::Int(i64::from(layout::game_types::ALL))),
                ("position", vec3([2., 1., 2.])),
                ("orientation", vec3([0., 0., 0.])),
                ("item", Value::Tag(Some(rifles()))),
            ]),
        )]
    );
    let err = s
        .place_item(&mut w, 2, &tag("itmc", r"fixture\other"), frame(2., 0.))
        .unwrap_err();
    assert!(err.contains("not an item this map's spawners use"), "{err}");
    assert_eq!(w.calls.len(), 1);
}

#[test]
fn a_teleporter_pair_shares_a_free_channel() {
    let mut s = kit_session();
    let mut w = Fake::default();
    // The map's teleporters use channels 1 and 3.
    s.place_teleporter(&mut w, 1, true, frame(1., 0.)).unwrap();
    s.place_teleporter(&mut w, 2, false, frame(2., 0.)).unwrap();
    s.place_teleporter(&mut w, 3, true, frame(3., 0.)).unwrap();
    let channel = |call: &Call| match call {
        Call::KitAdd(BlockKind::NetgameFlags, _, fields) => fields
            .iter()
            .find(|(n, _)| n == "identifier")
            .map(|(_, v)| v.clone()),
        call => panic!("{call:?}"),
    };
    assert_eq!(
        w.calls.iter().map(channel).collect::<Vec<_>>(),
        [Value::Int(0), Value::Int(0), Value::Int(2)].map(Some)
    );
    let kinds: Vec<_> = s
        .things()
        .iter()
        .filter(|t| !t.own)
        .map(|t| t.kind)
        .collect();
    assert_eq!(kinds, [ThingKind::Teleporter; 3]);
    // The map's own three flags come first, so the added are rows 3 to 5.
    assert!(matches!(w.calls[2], Call::KitAdd(_, 5, _)));
}

#[test]
fn a_block_with_no_room_left_refuses_the_add_and_changes_nothing() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.place_item(&mut w, 1, &rifles(), frame(1., 0.)).unwrap();
    let before = (s.document().clone(), w.calls.len());
    let err = s
        .place_item(&mut w, 2, &rifles(), frame(2., 0.))
        .unwrap_err();
    assert_eq!(err, "The kit has no room for another item spawner.");
    assert_eq!((s.document().clone(), w.calls.len()), before);
}

#[test]
fn a_map_without_the_kit_refuses_the_add_by_name() {
    let mut s = open(rows());
    let mut w = Fake::default();
    assert!(!s.has_kit());
    assert_eq!(s.kit_left(BlockKind::StartingLocations), None);
    let start = s.document().clone();
    for err in [
        s.place_spawn(&mut w, 1, 0, frame(1., 0.)).unwrap_err(),
        s.place_item(&mut w, 1, &rifles(), frame(1., 0.)).unwrap_err(),
        s.place_teleporter(&mut w, 1, true, frame(1., 0.)).unwrap_err(),
    ] {
        assert!(err.starts_with(kit::NO_KIT), "{err}");
    }
    assert!(w.calls.is_empty());
    assert_eq!(*s.document(), start);

    // A kit session's document loaded where there is no kit is refused too,
    // with the world as it was.
    let mut a = kit_session();
    let mut wa = Fake::default();
    a.place_spawn(&mut wa, 1, 0, frame(1., 0.)).unwrap();
    let saved = a.variant_form("Spawns");
    let err = s.load_variant(&mut w, 2, saved.clone()).unwrap_err();
    assert!(err.starts_with(kit::NO_KIT), "{err}");
    assert!(w.calls.is_empty());
    assert_eq!(s.document().ops, start.ops);

    // Where there is a kit the same document writes the row.
    let mut b = kit_session();
    let mut wb = Fake::default();
    b.load_variant(&mut wb, 2, saved).unwrap();
    assert_eq!(wb.calls, wa.calls);
}

#[test]
fn a_refused_kit_call_leaves_the_document_and_the_rows_as_they_were() {
    let mut s = kit_session();
    let mut w = Fake::default();
    w.refuse_at = Some(0);
    let err = s.place_spawn(&mut w, 1, 0, frame(1., 0.)).unwrap_err();
    assert_eq!(err, "named engine refusal");
    assert!(s.document().ops.is_empty());
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(2));
    // The row was never written, so it is written once, on the retry.
    s.place_spawn(&mut w, 2, 0, frame(1., 0.)).unwrap();
    assert_eq!(w.calls.len(), 1);
}

#[test]
fn goals_are_listed_by_the_game_types_that_read_them() {
    let s = kit_session();
    assert_eq!(s.goals(None), Goal::ALL);
    // Slayer reads neither a hill nor a territory.
    assert_eq!(s.goals(Some(2)), [Goal::Site]);
    assert_eq!(s.goals(Some(4)), [Goal::Hill, Goal::Site]);
    assert_eq!(s.goals(Some(8)), [Goal::Territory, Goal::Site]);
}

#[test]
fn a_goal_in_a_kit_session_takes_a_spare_flag_row() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.place_goal(&mut w, 1, Goal::Territory, frame(1., 0.)).unwrap();
    assert!(matches!(
        w.calls[..],
        [Call::KitAdd(BlockKind::NetgameFlags, 3, _)]
    ));
    // Without a kit the flag stays in the document alone, as it does today.
    let mut s = open(rows());
    let mut w = Fake::default();
    s.place_goal(&mut w, 1, Goal::Territory, frame(1., 0.)).unwrap();
    assert!(w.calls.is_empty());
}

#[test]
fn a_session_saves_without_its_kit_as_a_map_variant() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.place_spawn(&mut w, 1, 0, frame(1., 0.)).unwrap();
    assert!(s.document().recipes.contains(&forge_doc::kit()));
    let saved = s.variant_form("Spawns");
    assert!(!saved.recipes.contains(&forge_doc::kit()));
    assert_eq!(saved.tier(), Tier::Variant);
    assert_eq!(saved.ops, s.document().ops);
}

#[test]
fn the_room_after_a_block_is_its_run_of_spare_rows() {
    let block = BlockKind::StartingLocations;
    let spare = kit::inert_row(block);
    let rows = vec![spare.clone(), spare.clone(), vec![0; spare.len()], spare.clone()];
    let read = |n: u32| rows.get((n - 5) as usize).cloned();
    assert_eq!(kit::room(block, 5, read), 2);
    assert_eq!(kit::room(block, 5, |_| None), 0);
    // Never more than eight, and never past the engine's count.
    let all = |_: u32| Some(kit::inert_row(BlockKind::NetgameEquipment));
    assert_eq!(kit::room(BlockKind::NetgameEquipment, 10, all), 8);
    assert_eq!(kit::room(BlockKind::NetgameEquipment, 99, all), 1);
    // A map that has used its own entries' sentinels is not a spare row.
    assert_ne!(kit::inert_row(block), vec![0; spare.len()]);
}
