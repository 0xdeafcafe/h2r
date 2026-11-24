//! `Session::budget_for` over the invented map of `base_tests`: the
//! session's budget is 4, the weapons block holds 2 entries, the item
//! spawners block 1 (of at most 100) and the starting points 1 (no limit).
use crate::base_tests::{Fake, frame, open, rows, vec3, weapon};
use crate::{Binds, BlockKind, Budget, Endpoint, noun};

fn budget(left: u32, limit: u32, binds: Binds) -> Option<Budget> {
    Some(Budget { left, limit, binds })
}

#[test]
fn the_session_budget_binds_an_object_block_until_it_is_the_larger() {
    let mut s = open(rows());
    let mut w = Fake::default();
    assert_eq!(
        s.budget_for(BlockKind::Crates),
        budget(4, 4, Binds::Session)
    );
    s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    s.place(&mut w, 2, 0, frame(2., 0.)).unwrap();
    assert_eq!(
        s.budget_for(BlockKind::Crates),
        budget(2, 4, Binds::Session)
    );
    // The weapons block has room for 128; the session has two left.
    assert_eq!(
        s.budget_for(BlockKind::Weapons),
        budget(2, 4, Binds::Session)
    );
}

#[test]
fn a_block_with_no_session_budget_has_the_engines_count() {
    let mut s = open(rows());
    let mut w = Fake::default();
    assert_eq!(
        s.budget_for(BlockKind::NetgameEquipment),
        budget(99, 100, Binds::Block(BlockKind::NetgameEquipment))
    );
    // Removing an own entry gives its row back; a block with no limit has
    // no budget.
    s.remove(&mut w, 1, Endpoint::base(BlockKind::NetgameEquipment, 0))
        .unwrap();
    assert_eq!(
        s.budget_for(BlockKind::NetgameEquipment),
        budget(100, 100, Binds::Block(BlockKind::NetgameEquipment))
    );
    assert_eq!(s.budget_for(BlockKind::StartingLocations), None);
}

#[test]
fn a_move_or_a_field_costs_nothing_and_a_removal_gives_a_row_back() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let before = s.budget_for(BlockKind::Weapons);
    s.move_thing(&mut w, 1, weapon(1), frame(2., 0.5)).unwrap();
    s.set_field(&mut w, 2, weapon(1), "rotation", vec3([1., 0., 0.]))
        .unwrap();
    assert_eq!(s.budget_for(BlockKind::Weapons), before);
    s.remove(&mut w, 3, weapon(1)).unwrap();
    // The session counts only what was added.
    assert_eq!(s.budget_for(BlockKind::Weapons), before);
}

#[test]
fn a_full_limit_is_named_and_refuses_the_add() {
    let mut s = open(rows());
    let mut w = Fake::default();
    for tick in 1..=4 {
        s.place(&mut w, tick, 0, frame(tick as f32, 0.)).unwrap();
    }
    let full = s.budget_for(BlockKind::Crates).unwrap();
    assert_eq!((full.left, full.binds), (0, Binds::Session));
    assert_eq!(full.full(), "Objects are full (4).");
    let head = s.head();
    assert_eq!(
        s.place(&mut w, 5, 0, frame(5., 0.)).unwrap_err(),
        "Objects are full (4)."
    );
    assert_eq!(s.head(), head);
    // The block's own words, for the engine's limit.
    let spawners = s.budget_for(BlockKind::NetgameEquipment).unwrap();
    assert_eq!(
        Budget { left: 0, ..spawners }.full(),
        "Item spawners are full (100)."
    );
}

#[test]
fn the_nouns_agree_with_the_count() {
    let b = Budget {
        left: 1,
        limit: 100,
        binds: Binds::Block(BlockKind::NetgameEquipment),
    };
    assert_eq!((b.singular(), b.plural()), ("item spawner", "item spawners"));
    assert_eq!(noun(BlockKind::Crates), ("crate", "crates"));
    assert!((b.used() - 0.99).abs() < 1e-6);
    assert_eq!(Budget { left: 0, limit: 0, ..b }.used(), 1.0);
}
