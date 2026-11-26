//! The start gate's census: one fixture map, built from its file and from
//! the rows a game would read, gives one census
//! (docs/game/rules-start-gate.md, "The check").
use std::collections::BTreeMap;
use std::io::Cursor;

use forge_doc::{
    BlockKind, BuilderVersions, CensusOf, Edit, Endpoint, FieldWrite, FlagRow, Forge, ForgeDoc,
    Label, MapRef, MapRows, Op, OpId, Placement, Sha256, Value,
};
use rules::labels::{Census, Class, Member, ObjectType};
use scenario_edit::fixture::fixture;

fn doc(ops: Vec<Edit>) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256([0x11; 32]),
        },
        sources: vec![],
        title: "Census".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: ops
            .into_iter()
            .enumerate()
            .map(|(i, edit)| Op {
                id: OpId(i as u32 + 1),
                edit,
            })
            .collect(),
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

fn label(target: Endpoint, text: &str, number: Option<i32>) -> Edit {
    Edit::Forge(Forge::Label {
        target,
        label: Some(Label::new(text, number).unwrap()),
    })
}

fn base(block: BlockKind, index: u32) -> Endpoint {
    Endpoint::base(block, index)
}

/// The fixture's document: the first starting location is `"attack"` 0,
/// the second weapon `"core"`, a flag added like the map's one with team 0
/// is `"goal"`, and the second starting location is removed.
fn fixture_doc() -> ForgeDoc {
    doc(vec![
        label(base(BlockKind::StartingLocations, 0), "attack", Some(0)),
        label(base(BlockKind::Weapons, 1), "core", None),
        Edit::Placement(Placement::Add {
            block: BlockKind::NetgameFlags,
            like: Some(0),
            fields: vec![FieldWrite {
                field: "team".into(),
                value: Value::Int(0),
            }],
        }),
        label(Endpoint::Added(OpId(3)), "goal", None),
        Edit::Placement(Placement::Remove {
            block: BlockKind::StartingLocations,
            index: 1,
        }),
    ])
}

fn file_census(doc: Option<&ForgeDoc>) -> Census {
    let mut map = map::Map::from_reader(Cursor::new(fixture())).unwrap();
    Census::of_file(&mut map, doc).unwrap()
}

#[test]
fn a_map_file_and_the_game_rows_of_it_give_one_census() {
    let doc = fixture_doc();
    let from_file = file_census(Some(&doc));
    // The rows a game reads of the fixture's scenario, by hand: one flag
    // (type 2, the bomb spawn, team 1), two starting locations of team 1,
    // and the counts of every block. The game has no tag paths.
    let mut map = map::Map::from_reader(Cursor::new(fixture())).unwrap();
    let counts = forge_doc::rows_of_file(&mut map).unwrap().counts;
    assert_eq!(counts[&BlockKind::Weapons], 2);
    assert_eq!(counts[&BlockKind::StartingLocations], 2);
    assert_eq!(counts[&BlockKind::NetgameFlags], 1);
    let rows = MapRows {
        counts,
        flags: vec![FlagRow { kind: 2, team: 1 }],
        starts: vec![1, 1],
        tags: BTreeMap::from([("objects/test/blaster".to_string(), ObjectType::Weapon)]),
        sounds: None,
    };
    let from_game = Census::of_game(&rows, Some(&doc)).unwrap();
    // The file names its sounds (none here); the game has no tag paths.
    assert_eq!(from_file.sounds, Some(Default::default()));
    assert_eq!(from_game.sounds, None);
    assert_eq!(Census { sounds: None, ..from_file }, from_game);
    let member = |class, number, team| Member { class, number, team };
    assert_eq!(
        from_game.labels,
        BTreeMap::from([
            (
                "assault_bomb_spawn".to_string(),
                vec![member(Class::Flag, None, Some(1))]
            ),
            (
                "attack".to_string(),
                vec![member(Class::Spawn, Some(0), Some(1))]
            ),
            ("core".to_string(), vec![member(Class::Object, None, None)]),
            ("goal".to_string(), vec![member(Class::Flag, None, Some(0))]),
        ])
    );
}

#[test]
fn a_map_with_no_document_has_its_flags_built_in_labels_only() {
    let census = file_census(None);
    assert_eq!(census.labels.len(), 1);
    assert_eq!(
        census.labels["assault_bomb_spawn"],
        [Member {
            class: Class::Flag,
            number: None,
            team: Some(1)
        }]
    );
    assert_eq!(census.tags["objects/test/blaster"], ObjectType::Weapon);
}

#[test]
fn a_flags_own_label_replaces_its_built_in_one() {
    let doc = doc(vec![label(base(BlockKind::NetgameFlags, 0), "hill", Some(2))]);
    let census = file_census(Some(&doc));
    assert!(!census.labels.contains_key("assault_bomb_spawn"));
    assert_eq!(census.labels["hill"][0].number, Some(2));
}

#[test]
fn team_fields_outside_the_designators_are_no_team() {
    assert_eq!(forge_doc::designator(0), Some(0));
    assert_eq!(forge_doc::designator(7), Some(7));
    assert_eq!(forge_doc::designator(8), None);
    assert_eq!(forge_doc::designator(-1), None);
    assert_eq!(forge_doc::designator(0xffff), None);
}
