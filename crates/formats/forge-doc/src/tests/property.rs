//! `EntryOp::Property` and the `team` property (slice D4). All inputs are
//! invented.
use std::collections::BTreeMap;

use super::*;

const ITEMS: BlockKind = BlockKind::NetgameEquipment;
const WEAPONS: BlockKind = BlockKind::Weapons;
const PRINT: &str = "0123456789abcdef";

fn at(block: BlockKind, index: u32) -> Endpoint {
    Endpoint::Base {
        block,
        index,
        print: Some(serde_json::from_value(serde_json::Value::String(PRINT.into())).unwrap()),
    }
}

fn team(id: u32, target: Endpoint, team: Option<u8>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property {
            target,
            property: Property::Team(team),
        }),
    }
}

fn reset(id: u32, target: Endpoint) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Reset { target }),
    }
}

fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn limits() -> Limits {
    Limits {
        base_counts: BTreeMap::from([(ITEMS, 3), (WEAPONS, 2)]),
        ..Limits::default()
    }
}

#[test]
fn a_property_op_has_one_spelling_and_makes_a_format_3_document() {
    let op = team(1, at(ITEMS, 0), Some(2));
    let json = serde_json::to_value(&op.edit).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"entry": {"property": {
            "target": {"base": {"block": "netgame_equipment", "index": 0, "print": PRINT}},
            "property": {"team": 2}
        }}})
    );
    let cleared = serde_json::to_value(&team(1, at(ITEMS, 0), None).edit).unwrap();
    assert_eq!(
        cleared["entry"]["property"]["property"],
        serde_json::json!({"team": null})
    );
    let back: Edit = serde_json::from_value(json).unwrap();
    assert_eq!(back, op.edit);
    assert_eq!(with(vec![op]).format, FORMAT_ENTRIES);
    // An unknown property is no property.
    let bad = serde_json::json!({"entry": {"property": {
        "target": {"base": {"block": "netgame_equipment", "index": 0, "print": PRINT}},
        "property": {"weight": 1}
    }}});
    assert!(serde_json::from_value::<Edit>(bad).is_err());
}

#[test]
fn a_team_is_an_item_spawners_and_in_range() {
    let accepted = with(vec![
        team(1, at(ITEMS, 0), Some(7)),
        team(2, at(ITEMS, 1), None),
    ]);
    validate(&accepted, &limits()).unwrap();
    let refuse = |ops: Vec<Op>, expected: &str| {
        let error = validate(&with(ops), &limits()).unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "{error} did not mention {expected}"
        );
    };
    // A property on a kind that has none is refused.
    refuse(
        vec![team(1, at(WEAPONS, 0), Some(1))],
        "a team is an item spawner's",
    );
    refuse(vec![team(1, at(ITEMS, 0), Some(8))], "a team is 0 to 7");
    // A base entry is named with its print, as every entry op.
    let bare = Endpoint::base(ITEMS, 0);
    refuse(vec![team(1, bare, Some(1))], "named with its print");
}

#[test]
fn a_property_lowers_to_nothing_and_a_reset_drops_it() {
    let ops = vec![team(1, at(ITEMS, 0), Some(3))];
    let resolved = resolve(&ops, &limits().base_counts).unwrap();
    assert!(resolved.lowered.is_empty());
    assert_eq!(resolved.prints.len(), 1, "the print is still checked");
    assert_eq!(teams(&ops), BTreeMap::from([(Endpoint::base(ITEMS, 0), 3)]));

    // The last op wins; a clear leaves none; a reset covers what came before.
    let ops = vec![
        team(1, at(ITEMS, 0), Some(3)),
        team(2, at(ITEMS, 0), Some(5)),
        team(3, at(ITEMS, 1), Some(1)),
        team(4, at(ITEMS, 1), None),
    ];
    assert_eq!(teams(&ops), BTreeMap::from([(Endpoint::base(ITEMS, 0), 5)]));
    let mut ops = ops;
    ops.push(reset(5, at(ITEMS, 0)));
    assert!(teams(&ops).is_empty());
    validate(&with(ops), &limits()).unwrap();
}

#[test]
fn the_census_reports_a_spawners_team_at_its_final_index() {
    let rows = MapRows {
        counts: BTreeMap::from([(ITEMS, 3)]),
        ..MapRows::default()
    };
    let goal = |id, target| Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Label {
            target,
            label: Some(Label::new("goal", None).unwrap()),
        }),
    };
    // Spawner 0 is removed, so spawner 2 moves into its row, index 0.
    let doc = with(vec![
        goal(1, Endpoint::base(ITEMS, 2)),
        team(2, at(ITEMS, 2), Some(4)),
        goal(3, Endpoint::base(ITEMS, 1)),
        Op {
            id: OpId(4),
            edit: Edit::Entry(EntryOp::Remove {
                target: at(ITEMS, 0),
            }),
        },
    ]);
    let things = labelled(&rows, Some(&doc)).unwrap();
    let teams: Vec<_> = things.iter().map(|t| (t.block, t.index, t.team)).collect();
    assert_eq!(teams, [(ITEMS, 0, Some(4)), (ITEMS, 1, None)]);
    // With no document there are no teams.
    assert!(labelled(&rows, None).unwrap().is_empty());
}
