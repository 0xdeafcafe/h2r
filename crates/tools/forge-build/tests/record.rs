//! A built map carries its spawners' properties with no retail field in
//! the Forge record at its tail (docs/game/forge-map-variants.md, "The
//! Forge record"), before the areas record, so a match on the variant reads
//! them with no document.
use std::io::Cursor;

use content::Sha256;
use forge_build::{Built, VERSIONS, build};
use forge_doc::record::ForgeRecord;
use forge_doc::{
    BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, MapRef, Op, OpId, Placement,
    Property, Value,
};
use scenario_edit::{MapSource, fixture};

const ITEMS: BlockKind = BlockKind::NetgameEquipment;
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
        title: "Record".into(),
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

fn add(id: u32, block: BlockKind, fields: Vec<FieldWrite>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add { block, like: None, fields }),
    }
}

fn property(id: u32, target: Endpoint, property: Property) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(EntryOp::Property { target, property }),
    }
}

fn items(built: &Built) -> u32 {
    let mut map = map::Map::from_reader(Cursor::new(built.map_bytes())).unwrap();
    scenario_edit::Scenario::read(&mut map).unwrap().block(ITEMS).len() as u32
}

#[test]
fn an_added_spawners_clips_and_place_at_start_are_in_the_maps_forge_record() {
    let source = source();
    let plain = materialise(&source, vec![add(1, ITEMS, vec![])]);
    let read = |b: &Built| ForgeRecord::read_tail(&mut Cursor::new(b.map_bytes())).unwrap();
    assert_eq!(read(&plain), None, "a document with no property has no record");

    let added = Endpoint::Added(OpId(1));
    let built = materialise(
        &source,
        vec![
            add(1, ITEMS, vec![]),
            property(2, added, Property::SpareClips(Some(3))),
            property(3, added, Property::PlaceAtStart(Some(false))),
        ],
    );
    let row = items(&built) - 1;
    let record = read(&built).expect("the record");
    assert_eq!(record.spare_clips.into_iter().collect::<Vec<_>>(), [(row, 3)]);
    assert_eq!(record.held_back.into_iter().collect::<Vec<_>>(), [row]);
    assert!(record.weights.is_empty());

    // With a shown flag too, the areas record follows and both read.
    let f = |v| F32::new(v).unwrap();
    let both = materialise(
        &source,
        vec![
            add(1, ITEMS, vec![]),
            property(2, added, Property::SpareClips(Some(3))),
            add(
                4,
                FLAGS,
                vec![
                    FieldWrite { field: "type".into(), value: Value::Int(7) },
                    FieldWrite {
                        field: "position".into(),
                        value: Value::Vec3([f(2.0), f(0.0), f(1.0)]),
                    },
                ],
            ),
        ],
    );
    assert_eq!(read(&both).unwrap().spare_clips.len(), 1);
    let areas = game_markers::record::read_tail(&mut Cursor::new(both.map_bytes())).unwrap();
    assert_eq!(areas.map(|rows| rows.len()), Some(1));
}
