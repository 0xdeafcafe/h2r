//! `EntryOp::Property` and the `team` property (slice D4). All inputs are
//! invented.
use std::collections::{BTreeMap, BTreeSet};

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

// The `games` property of an object (slice G1).

const SCENERY: BlockKind = BlockKind::Scenery;
const MACHINES: BlockKind = BlockKind::Machines;
const CTF: i32 = 1;
const SLAYER: i32 = 2;

fn games(id: u32, target: Endpoint, set: Option<&[i32]>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property {
            target,
            property: Property::Games(set.map(games::GameSet::of)),
        }),
    }
}

fn object_limits() -> Limits {
    Limits {
        base_counts: BTreeMap::from([(SCENERY, 3), (MACHINES, 2), (BlockKind::Crates, 1)]),
        ..Limits::default()
    }
}

fn add(id: u32, block: BlockKind) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block,
            like: None,
            fields: vec![],
        }),
    }
}

#[test]
fn a_games_property_has_one_spelling() {
    let op = games(1, at(SCENERY, 0), Some(&[SLAYER, CTF]));
    let json = serde_json::to_value(&op.edit).unwrap();
    assert_eq!(
        json["entry"]["property"]["property"],
        serde_json::json!({"games": [1, 2]})
    );
    assert_eq!(serde_json::from_value::<Edit>(json).unwrap(), op.edit);
    let cleared = serde_json::to_value(&games(1, at(SCENERY, 0), None).edit).unwrap();
    assert_eq!(
        cleared["entry"]["property"]["property"],
        serde_json::json!({"games": null})
    );
    assert_eq!(with(vec![op]).format, FORMAT_ENTRIES);
}

#[test]
fn game_types_are_an_objects_and_a_proper_nonempty_set() {
    let accepted = with(vec![
        games(1, at(SCENERY, 0), Some(&[CTF])),
        games(2, at(MACHINES, 1), Some(&[SLAYER, 3])),
        games(3, at(BlockKind::Crates, 0), None),
    ]);
    validate(&accepted, &object_limits()).unwrap();
    let refuse = |ops: Vec<Op>, expected: &str| {
        let error = validate(&with(ops), &object_limits())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(expected),
            "{error} did not mention {expected}"
        );
    };
    refuse(
        vec![games(1, at(ITEMS, 0), Some(&[CTF]))],
        "a scenery, crate or machine placement's",
    );
    refuse(
        vec![games(1, at(SCENERY, 0), Some(&[]))],
        "needs a game type",
    );
    refuse(
        vec![games(1, at(SCENERY, 0), Some(&[1, 2, 3, 4, 7, 8, 9]))],
        "clear it",
    );
}

#[test]
fn an_entry_with_game_types_has_no_link_or_label() {
    let device = |id, from, to| Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Link(Link::Device { from, to })),
    };
    let label = |id, target| Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Label {
            target,
            label: Some(Label::new("goal", None).unwrap()),
        }),
    };
    let refuse = |ops: Vec<Op>, expected: &str| {
        let error = validate(&with(ops), &object_limits())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(expected),
            "{error} did not mention {expected}"
        );
    };
    let m0 = Endpoint::base(MACHINES, 0);
    let m1 = Endpoint::base(MACHINES, 1);
    refuse(
        vec![device(1, m0, m1), games(2, at(MACHINES, 0), Some(&[CTF]))],
        "a linked entry is in every game type",
    );
    refuse(
        vec![games(1, at(MACHINES, 1), Some(&[CTF])), device(2, m0, m1)],
        "clear its game types first",
    );
    refuse(
        vec![
            label(1, Endpoint::base(SCENERY, 0)),
            games(2, at(SCENERY, 0), Some(&[CTF])),
        ],
        "a labelled entry is in every game type",
    );
    refuse(
        vec![
            games(1, at(SCENERY, 0), Some(&[CTF])),
            label(2, Endpoint::base(SCENERY, 0)),
        ],
        "clear its game types first",
    );
    // A cleared set, or one a reset covers, frees the entry.
    validate(
        &with(vec![
            games(1, at(MACHINES, 1), Some(&[CTF])),
            games(2, at(MACHINES, 1), None),
            device(3, m0, m1),
        ]),
        &object_limits(),
    )
    .unwrap();
    validate(
        &with(vec![
            games(1, at(MACHINES, 1), Some(&[CTF])),
            reset(2, at(MACHINES, 1)),
            device(3, m0, m1),
        ]),
        &object_limits(),
    )
    .unwrap();
}

#[test]
fn object_games_folds_as_teams_do() {
    let ops = vec![
        games(1, at(SCENERY, 0), Some(&[CTF])),
        games(2, at(SCENERY, 0), Some(&[SLAYER])),
        games(3, at(SCENERY, 1), Some(&[CTF])),
        games(4, at(SCENERY, 1), None),
        games(5, at(SCENERY, 2), Some(&[CTF])),
        reset(6, at(SCENERY, 2)),
    ];
    assert_eq!(
        object_games(&ops),
        BTreeMap::from([(Endpoint::base(SCENERY, 0), games::GameSet::of(&[SLAYER]))])
    );
    // A property lowers to nothing.
    assert!(
        resolve(&ops, &object_limits().base_counts)
            .unwrap()
            .lowered
            .is_empty()
    );
}

#[test]
fn a_document_for_a_game_removes_the_objects_of_other_game_types_last() {
    let doc = with(vec![
        games(1, at(SCENERY, 0), Some(&[CTF])),
        add(2, SCENERY),
        games(3, Endpoint::Added(OpId(2)), Some(&[SLAYER])),
        games(4, at(SCENERY, 1), Some(&[CTF, SLAYER])),
    ]);
    let limits = object_limits();
    validate(&doc, &limits).unwrap();

    // Slayer: scenery 0 goes, by an entry removal after every op, and the
    // op that set its game types is left out, so the document does not grow.
    let slayer = for_game(&doc, SLAYER, &limits.base_counts).unwrap();
    validate(&slayer, &limits).unwrap();
    assert_eq!(slayer.ops.len(), doc.ops.len());
    assert_eq!(
        slayer.ops.last().unwrap(),
        &Op {
            id: OpId(5),
            edit: Edit::Entry(EntryOp::Remove {
                target: at(SCENERY, 0)
            }),
        }
    );
    assert!(!slayer.ops.iter().any(|op| op.id == OpId(1)));
    assert_eq!(slayer.title, doc.title);

    // Capture the Flag: the added scenery goes.
    let ctf = for_game(&doc, CTF, &limits.base_counts).unwrap();
    validate(&ctf, &limits).unwrap();
    assert_eq!(
        ctf.ops.last().unwrap().edit,
        Edit::Entry(EntryOp::Remove {
            target: Endpoint::Added(OpId(2))
        })
    );

    // Oddball: both go; scenery 1, which has Slayer and CTF, goes too.
    let oddball = for_game(&doc, 3, &limits.base_counts).unwrap();
    validate(&oddball, &limits).unwrap();
    let removed: Vec<_> = oddball
        .ops
        .iter()
        .filter_map(|op| match &op.edit {
            Edit::Entry(EntryOp::Remove { target }) => Some(target.identity()),
            _ => None,
        })
        .collect();
    assert_eq!(removed.len(), 3);

    // A document without object game types is as it is.
    let plain = with(vec![add(1, SCENERY)]);
    assert_eq!(
        for_game(&plain, SLAYER, &limits.base_counts).unwrap(),
        plain
    );
}

#[test]
fn an_object_already_removed_is_not_removed_again() {
    let doc = with(vec![
        add(1, SCENERY),
        games(2, Endpoint::Added(OpId(1)), Some(&[CTF])),
        Op {
            id: OpId(3),
            edit: Edit::Placement(Placement::Remove {
                block: SCENERY,
                index: 3,
            }),
        },
    ]);
    let limits = object_limits();
    validate(&doc, &limits).unwrap();
    let slayer = for_game(&doc, SLAYER, &limits.base_counts).unwrap();
    validate(&slayer, &limits).unwrap();
    assert_eq!(slayer.ops.len(), 2);
    assert_eq!(slayer.format, slayer.required_format());
}

#[test]
fn a_document_with_object_game_types_expects_no_map() {
    let mut doc = with(vec![games(1, at(SCENERY, 0), Some(&[CTF]))]);
    doc.expect = Some(Expected {
        map_id: MapId([1; 32]),
        geometry: Sha256([2; 32]),
    });
    let error = validate(&doc, &object_limits()).unwrap_err().to_string();
    assert!(error.contains("expects no map"), "{error}");
}

fn clips(id: u32, target: Endpoint, clips: Option<u8>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property {
            target,
            property: Property::SpareClips(clips),
        }),
    }
}

#[test]
fn spare_clips_are_an_item_spawners_in_range_and_the_last_op_wins() {
    let op = clips(1, at(ITEMS, 0), Some(2));
    let json = serde_json::to_value(&op.edit).unwrap();
    assert_eq!(
        json["entry"]["property"]["property"],
        serde_json::json!({"spare_clips": 2})
    );
    validate(
        &with(vec![op, clips(2, at(ITEMS, 1), Some(SPARE_CLIPS_MAX))]),
        &limits(),
    )
    .unwrap();
    let refuse = |ops: Vec<Op>, expected: &str| {
        let error = validate(&with(ops), &limits()).unwrap_err().to_string();
        assert!(error.contains(expected), "{error} did not mention {expected}");
    };
    refuse(
        vec![clips(1, at(WEAPONS, 0), Some(1))],
        "spare clips are an item spawner's",
    );
    refuse(
        vec![clips(1, at(ITEMS, 0), Some(SPARE_CLIPS_MAX + 1))],
        "spare clips are 0 to 10",
    );

    // Teams and clips are read apart; a clear leaves none; a reset covers both.
    let mut ops = vec![
        clips(1, at(ITEMS, 0), Some(3)),
        team(2, at(ITEMS, 0), Some(1)),
        clips(3, at(ITEMS, 0), Some(0)),
        clips(4, at(ITEMS, 1), Some(5)),
        clips(5, at(ITEMS, 1), None),
    ];
    assert_eq!(spare_clips(&ops), BTreeMap::from([(Endpoint::base(ITEMS, 0), 0)]));
    assert_eq!(teams(&ops), BTreeMap::from([(Endpoint::base(ITEMS, 0), 1)]));
    ops.push(reset(6, at(ITEMS, 0)));
    assert!(spare_clips(&ops).is_empty());
    assert!(resolve(&ops, &limits().base_counts).unwrap().lowered.is_empty());
}

fn weight(id: u32, target: Endpoint, weight: Option<SpawnWeight>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property {
            target,
            property: Property::Weight(weight),
        }),
    }
}

#[test]
fn a_weight_is_a_starting_points_and_the_last_op_wins() {
    const STARTS: BlockKind = BlockKind::StartingLocations;
    let limits = Limits {
        base_counts: BTreeMap::from([(ITEMS, 3), (STARTS, 2)]),
        ..Limits::default()
    };
    let op = weight(1, at(STARTS, 0), Some(SpawnWeight::High));
    assert_eq!(
        serde_json::to_value(&op.edit).unwrap()["entry"]["property"]["property"],
        serde_json::json!({"weight": "high"})
    );
    validate(&with(vec![op.clone()]), &limits).unwrap();
    let error = validate(&with(vec![weight(1, at(ITEMS, 0), Some(SpawnWeight::Low))]), &limits)
        .unwrap_err()
        .to_string();
    assert!(error.contains("a weight is a starting point's"), "{error}");
    let mut ops = vec![
        op,
        weight(2, at(STARTS, 1), Some(SpawnWeight::Low)),
        weight(3, at(STARTS, 1), None),
    ];
    assert_eq!(
        spawn_weights(&ops),
        BTreeMap::from([(Endpoint::base(STARTS, 0), SpawnWeight::High)])
    );
    ops.push(reset(4, at(STARTS, 0)));
    assert!(spawn_weights(&ops).is_empty());
}

fn at_start(id: u32, target: Endpoint, at_start: Option<bool>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property {
            target,
            property: Property::PlaceAtStart(at_start),
        }),
    }
}

#[test]
fn place_at_start_no_is_an_item_spawners_and_yes_is_no_property() {
    let op = at_start(1, at(ITEMS, 2), Some(false));
    assert_eq!(
        serde_json::to_value(&op.edit).unwrap()["entry"]["property"]["property"],
        serde_json::json!({"place_at_start": false})
    );
    validate(&with(vec![op.clone()]), &limits()).unwrap();
    let refuse = |ops: Vec<Op>, expected: &str| {
        let error = validate(&with(ops), &limits()).unwrap_err().to_string();
        assert!(error.contains(expected), "{error} did not mention {expected}");
    };
    refuse(vec![at_start(1, at(WEAPONS, 0), Some(false))], "an item spawner's");
    refuse(vec![at_start(1, at(ITEMS, 0), Some(true))], "the default: clear it");
    let mut ops = vec![op, at_start(2, at(ITEMS, 0), Some(false)), at_start(3, at(ITEMS, 0), None)];
    assert_eq!(held_at_start(&ops), BTreeSet::from([Endpoint::base(ITEMS, 2)]));
    ops.push(reset(4, at(ITEMS, 2)));
    assert!(held_at_start(&ops).is_empty());
}

#[test]
fn the_forge_record_holds_each_property_at_its_built_index_and_round_trips() {
    use crate::record::{ForgeRecord, RecordError};
    const STARTS: BlockKind = BlockKind::StartingLocations;
    let limits = Limits {
        base_counts: BTreeMap::from([(ITEMS, 3), (STARTS, 2)]),
        ..Limits::default()
    };
    // Item 0 is removed, so item 2 is the built map's item 1.
    let doc = with(vec![
        clips(1, at(ITEMS, 2), Some(4)),
        clips(2, at(ITEMS, 0), Some(1)),
        at_start(3, at(ITEMS, 2), Some(false)),
        weight(4, at(STARTS, 1), Some(SpawnWeight::Low)),
        Op {
            id: OpId(5),
            edit: Edit::Entry(EntryOp::Remove { target: at(ITEMS, 0) }),
        },
    ]);
    validate(&doc, &limits).unwrap();
    let record = ForgeRecord::of(&doc, &limits.base_counts).unwrap();
    assert_eq!(
        record,
        ForgeRecord {
            spare_clips: BTreeMap::from([(1, 4)]),
            weights: BTreeMap::from([(1, SpawnWeight::Low)]),
            held_back: BTreeSet::from([1]),
        }
    );
    // At a map's tail, before an areas record, it reads back whole.
    let mut file = vec![9u8; 64];
    file.extend(record.encode());
    file.extend(game_markers::record::encode(&[]));
    let back = ForgeRecord::read_tail(&mut std::io::Cursor::new(&file)).unwrap();
    assert_eq!(back, Some(record.clone()));
    assert_eq!(
        game_markers::record::read_tail(&mut std::io::Cursor::new(&file)).unwrap(),
        Some(Vec::new())
    );
    // A map with no record has none; a damaged one is refused.
    assert_eq!(ForgeRecord::read_tail(&mut std::io::Cursor::new(vec![9u8; 64])).unwrap(), None);
    let mut bytes = record.encode();
    let body = bytes.len() - 8;
    bytes[body - 5] = 11;
    assert_eq!(ForgeRecord::decode(&bytes[..body]), Err(RecordError::Checksum));
    assert!(ForgeRecord::default().encode().len() == 19);
}
