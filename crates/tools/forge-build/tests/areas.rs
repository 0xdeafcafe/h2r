//! A built map carries the flags the document shows in play in its areas
//! record, as a Studio map carries its areas (docs/game/gametype-marker-forms.md,
//! "When a form shows"): the game reads the record when it loads the map.
use std::io::Cursor;

use content::Sha256;
use forge_build::{Built, VERSIONS, build};
use forge_doc::{
    BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, MapRef, Op, OpId, Placement,
    Value,
};
use game_markers::record::{self, Row};
use scenario_edit::{MapSource, fixture};

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn source() -> MapSource {
    let mut source = MapSource::file("invented/fixture.map");
    source.bytes = Some(fixture::fixture_at_end().into());
    source
}

fn materialise(source: &MapSource, ops: Vec<Op>) -> Built {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(source.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Shown".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops,
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    build(&doc, |name| (name == "fixture").then(|| source.clone())).unwrap()
}

/// A new netgame flag of `kind` at an invented position.
fn flag(id: u32, kind: i64, x: f32) -> Op {
    let f = |v| F32::new(v).unwrap();
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block: FLAGS,
            like: None,
            fields: vec![
                FieldWrite {
                    field: "type".into(),
                    value: Value::Int(kind),
                },
                FieldWrite {
                    field: "position".into(),
                    value: Value::Vec3([f(x), f(0.0), f(1.0)]),
                },
            ],
        }),
    }
}

fn show(id: u32, target: Endpoint, on: bool) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Show { target, on }),
    }
}

fn record_of(built: &Built) -> Option<Vec<Row>> {
    record::read_tail(&mut Cursor::new(built.map_bytes())).unwrap()
}

#[test]
fn a_flag_the_document_added_is_shown_in_the_maps_record_and_a_hidden_one_is_not() {
    let source = source();
    let plain = materialise(&source, vec![]);
    assert_eq!(record_of(&plain), None, "a document with no flag has no record");

    // A teleporter sender (type 7) the document added has a play form: shown.
    let built = materialise(&source, vec![flag(1, 7, 2.0)]);
    let rows = record_of(&built).expect("the added flag is shown");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(row.show);
    assert_eq!((row.flag_type, row.area.clone()), (7, None));
    assert_eq!(row.at, [2.0, 0.0, 1.0]);
    let flags = built_flags(&built);
    assert_eq!(row.index as usize, flags - 1, "the last flag the map has");

    // A flag with no marker (type 5 has no label) has no play form to show.
    assert_eq!(record_of(&materialise(&source, vec![flag(1, 5, 2.0)])), None);

    // The op turns the added flag off: no row says anything.
    let off = materialise(
        &source,
        vec![flag(1, 7, 2.0), show(2, Endpoint::Added(OpId(1)), false)],
    );
    assert_eq!(record_of(&off), None);
    // And the two builds are the bytes the document names.
    let again = materialise(&source, vec![flag(1, 7, 2.0)]);
    assert_eq!(built.map_bytes(), again.map_bytes());
}

fn built_flags(built: &Built) -> usize {
    let mut map = map::Map::from_reader(Cursor::new(built.map_bytes())).unwrap();
    scenario_edit::Scenario::read(&mut map)
        .unwrap()
        .block(FLAGS)
        .len()
}
