//! **Rebuild map** and the reload after an AI ground build over the fake
//! world and invented map of `base_tests`: the play copy keeps every row
//! where the session has it, and a session that goes on over the copy
//! places nothing the copy already holds and keeps its undo.
use std::collections::BTreeMap;

use forge_doc::{Edit, EntryOp, GroundRecipe, Value};

use crate::base_tests::{Call, Fake, crate_tag, frame, open, rows, weapon};
use crate::{BlockKind, CopyKind, Endpoint, Frame, Reopened, Session};

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

/// The spare rows a copy built with the kit has past its rows.
fn spare(starts: u32) -> Option<BTreeMap<BlockKind, u32>> {
    Some(BTreeMap::from([
        (BlockKind::StartingLocations, starts),
        (BlockKind::NetgameFlags, 3),
        (BlockKind::NetgameEquipment, 1),
    ]))
}

#[test]
fn a_session_with_no_edits_has_nothing_to_rebuild() {
    assert!(!open(rows()).rebuild_needed());
    let mut s = kit_session();
    assert!(!s.rebuild_needed());
    let mut w = Fake::default();
    s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    assert!(s.rebuild_needed());
    // Undone, the document is the map again.
    s.undo(&mut w, 2).unwrap();
    assert!(!s.rebuild_needed());
}

#[test]
fn the_play_copy_keeps_a_removed_own_entry_in_its_row() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.remove(&mut w, 1, weapon(1)).unwrap();
    s.remove(&mut w, 2, Endpoint::base(BlockKind::NetgameFlags, 1))
        .unwrap();
    let built = s.rebuild_form().unwrap();
    // The weapon's row makes no object; the flag's removal is left out.
    let edits: Vec<_> = built.ops.iter().map(|op| &op.edit).collect();
    assert!(!edits.iter().any(|e| matches!(e, Edit::Entry(EntryOp::Remove { .. }))));
    assert!(edits.iter().any(|e| matches!(e,
        Edit::Entry(EntryOp::Set { target, write })
            if target.identity() == weapon(1).identity()
                && write.field == "palette index"
                && write.value == Value::Tag(None))));
    assert_eq!(built.ops.len(), s.document().ops.len() - 1);
    assert!(built.recipes.contains(&forge_doc::kit()));
    assert!(built.expect.is_none());
}

#[test]
fn reopening_over_the_rebuilt_map_places_nothing_the_copy_holds() {
    let mut s = kit_session();
    let mut w = Fake::default();
    let crate_op = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    s.place_spawn(&mut w, 2, 0, frame(2., 0.)).unwrap();
    s.remove(&mut w, 3, weapon(1)).unwrap();
    let moved = frame(-2., 0.5);
    s.move_thing(&mut w, 4, weapon(0), moved).unwrap();
    assert!(s.rebuild_needed());
    let doc = s.document().clone();
    let log = s.log().to_vec();

    let built = s.rebuild_form().unwrap();
    // The crate is the first row of the map's empty crate block.
    assert_eq!(
        s.baked_objects(&built).unwrap(),
        [(crate_op, BlockKind::Crates, 0)]
    );
    let mut copy = Fake::default();
    let found = Reopened {
        handles: BTreeMap::from([(crate_op, 77)]),
        room: spare(1),
    };
    s.reopen(&mut copy, &built, CopyKind::Rebuild, found).unwrap();
    // The crate, the spawn and the move are in the copy already; only the
    // removal the copy kept in its row is made inert again.
    assert_eq!(copy.calls, [Call::BaseInert(BlockKind::Weapons, 1)]);
    assert_eq!(s.objects().len(), 1);
    assert_eq!(s.objects()[0].handle, 77);
    assert_eq!(s.objects()[0].item.tag, crate_tag());
    assert_eq!(s.document(), &doc);
    assert_eq!(s.log(), &log[..]);
    assert!(!s.rebuild_needed());
    // The kit's room is the copy's spare rows.
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(1));

    // Undo goes back over the rebuild, step by step, on the copy's rows.
    s.undo(&mut copy, 5).unwrap();
    assert_eq!(
        copy.calls.last(),
        Some(&Call::BaseMove(
            BlockKind::Weapons,
            0,
            Frame {
                position: [-3., 0., 0.],
                rotation: [0.; 3],
            }
        ))
    );
    s.undo(&mut copy, 6).unwrap();
    assert!(matches!(
        copy.calls.last(),
        Some(Call::BaseRestore(BlockKind::Weapons, 1, _))
    ));
    s.undo(&mut copy, 7).unwrap();
    assert_eq!(
        copy.calls.last(),
        Some(&Call::KitDrop(BlockKind::StartingLocations, 1))
    );
    s.undo(&mut copy, 8).unwrap();
    assert_eq!(copy.calls.last(), Some(&Call::Delete(77)));
    assert!(s.rebuild_needed());
}

#[test]
fn an_added_object_the_reloaded_game_did_not_make_is_placed_again() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let built = s.rebuild_form().unwrap();
    let mut copy = Fake::default();
    let found = Reopened {
        handles: BTreeMap::new(),
        room: spare(2),
    };
    s.reopen(&mut copy, &built, CopyKind::Rebuild, found).unwrap();
    assert_eq!(copy.calls, [Call::Place(1, frame(1., 0.))]);
    assert_eq!(s.objects()[0].handle, 1);
}

#[test]
fn a_ground_copy_is_loaded_until_the_ground_is_stale_and_has_no_kit_room() {
    let mut s = kit_session();
    s.set_ground_recipe(Some(GroundRecipe {
        kit_version: 1,
        home: 0,
        probe: false,
    }));
    let mut w = Fake::default();
    s.place_spawn(&mut w, 1, 0, frame(2., 0.)).unwrap();
    let built = s.ground_copy_form().unwrap();
    assert!(!built.recipes.contains(&forge_doc::kit()));
    assert!(!s.ground_loaded());
    let mut copy = Fake::default();
    s.reopen(&mut copy, &built, CopyKind::Ground, Reopened::default())
        .unwrap();
    assert!(copy.calls.is_empty());
    assert!(s.ground_loaded());
    // The ground's map has no spare rows: the kit's adds wait for Rebuild map.
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(0));
    assert!(s.place_spawn(&mut copy, 2, 0, frame(3., 0.)).is_err());
    // It is not what Rebuild map builds, so a rebuild has work to do.
    assert!(s.rebuild_needed());
    let built = s.rebuild_form().unwrap();
    let found = Reopened {
        room: spare(1),
        ..Default::default()
    };
    s.reopen(&mut copy, &built, CopyKind::Rebuild, found).unwrap();
    assert!(!s.ground_loaded());
    assert_eq!(s.kit_left(BlockKind::StartingLocations), Some(1));
}

#[test]
fn a_refused_reopen_leaves_the_session_as_it_was() {
    let mut s = kit_session();
    let mut w = Fake::default();
    s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let before = s.objects().to_vec();
    let built = s.rebuild_form().unwrap();
    let mut copy = Fake {
        refuse_at: Some(0),
        ..Fake::default()
    };
    let found = Reopened {
        handles: BTreeMap::new(),
        room: spare(2),
    };
    assert!(s.reopen(&mut copy, &built, CopyKind::Rebuild, found).is_err());
    assert_eq!(s.objects(), &before[..]);
    assert!(s.rebuild_needed());
}
