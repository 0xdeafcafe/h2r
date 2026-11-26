//! G1 acceptance on the invented scenario-edit fixture: an object with a
//! `games` property is left out of a map built for an engine its set
//! lacks, and the package's identity names the engine only then
//! (docs/game/forge-map-variants.md, "Game types").
use std::io::Cursor;

use content::{MapId, Sha256};
use forge_build::{Built, Error, VERSIONS, base_print, build, build_for, map_id};
use forge_doc::{
    BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, MapRef, Op, OpId, Placement,
    Property, Value, games::GameSet,
};
use map::Map;
use scenario_edit::{MapSource, Scenario, fixture};

const SCENERY: BlockKind = BlockKind::Scenery;
const CTF: i32 = 1;
const SLAYER: i32 = 2;

fn source() -> MapSource {
    let mut source = MapSource::file("invented/fixture.map");
    source.bytes = Some(fixture::fixture_at_end().into());
    source
}

fn doc(source: &MapSource, edits: Vec<Edit>) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(source.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Variant".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: edits
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

fn materialise(source: &MapSource, doc: &ForgeDoc, engine: Option<i32>) -> Result<Built, Error> {
    build_for(doc, engine, |name| {
        (name == "fixture").then(|| source.clone())
    })
}

fn f(x: f32) -> F32 {
    F32::new(x).unwrap()
}

/// The fixture's one scenery placement, named with its print.
fn own_scenery(source: &MapSource) -> Endpoint {
    let mut map = source.open().unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    Endpoint::Base {
        block: SCENERY,
        index: 0,
        print: Some(base_print(&map, &scenario, SCENERY, 0).unwrap()),
    }
}

fn games(target: Endpoint, set: Option<GameSet>) -> Edit {
    Edit::Entry(EntryOp::Property {
        target,
        property: Property::Games(set),
    })
}

/// A copy of the fixture's scenery at an invented position.
fn add_scenery(x: f32) -> Edit {
    Edit::Placement(Placement::Add {
        block: SCENERY,
        like: Some(0),
        fields: vec![FieldWrite {
            field: "position".into(),
            value: Value::Vec3([f(x), f(2.0), f(3.0)]),
        }],
    })
}

/// The scenery positions of a built map, in block order.
fn scenery(built: &Built) -> Vec<[f32; 3]> {
    let mut map = Map::from_reader(Cursor::new(built.map_bytes())).unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    (0..scenario.block(SCENERY).len())
        .map(|i| {
            scenario
                .get(SCENERY, i, "position")
                .unwrap()
                .as_vec3()
                .unwrap()
        })
        .collect()
}

/// The fixture's scenery is Capture the Flag's only; a new scenery at x 9
/// is in every game type; a second new one at x 11 is Slayer's.
fn by_game_type(source: &MapSource) -> ForgeDoc {
    doc(
        source,
        vec![
            games(own_scenery(source), Some(GameSet::of(&[CTF]))),
            add_scenery(9.0),
            add_scenery(11.0),
            games(Endpoint::Added(OpId(3)), Some(GameSet::of(&[SLAYER]))),
        ],
    )
}

#[test]
fn a_build_for_slayer_lacks_the_ctf_only_scenery_and_has_the_rest() {
    let source = source();
    let doc = by_game_type(&source);
    // The fixture's scenery stands at [5, 5, 5].
    let fixture = [5.0, 5.0, 5.0];

    // Capture the Flag has the fixture's scenery and the every-game one,
    // not Slayer's.
    let ctf = materialise(&source, &doc, Some(CTF)).unwrap();
    assert_eq!(scenery(&ctf), vec![fixture, [9.0, 2.0, 3.0]]);

    // Slayer has the every-game scenery and its own, not the CTF-only
    // one; its removal swap-removes, as the editor's does, so Slayer's
    // scenery takes slot 0.
    let slayer = materialise(&source, &doc, Some(SLAYER)).unwrap();
    assert_eq!(scenery(&slayer), vec![[11.0, 2.0, 3.0], [9.0, 2.0, 3.0]]);
    assert_eq!(
        slayer.prints.len(),
        1,
        "the removed entry's print is checked"
    );
}

#[test]
fn the_engine_is_in_the_identity_only_of_a_document_with_object_game_types() {
    let source = source();
    let doc = by_game_type(&source);
    let hash = doc.id().unwrap();
    let slayer = materialise(&source, &doc, Some(SLAYER)).unwrap();
    let ctf = materialise(&source, &doc, Some(CTF)).unwrap();
    assert_eq!(slayer.manifest().forge[0].game, Some(SLAYER as u8));
    assert_eq!(ctf.manifest().forge[0].game, Some(CTF as u8));
    assert_eq!(
        slayer.manifest().map_id(),
        MapId::forge_game(doc.base.sha256, hash, Some(SLAYER as u8))
    );
    assert_eq!(
        slayer.manifest().map_id(),
        map_id(&doc, Some(SLAYER)).unwrap()
    );
    assert_ne!(slayer.manifest().map_id(), ctf.manifest().map_id());
    assert_ne!(
        slayer.manifest().map_id(),
        MapId::forge(doc.base.sha256, hash)
    );
    // The document in the package is the one authored, every op kept.
    assert_eq!(slayer.manifest().forge[0].document, hash);

    // A document without object game types keeps its id whatever engine
    // it is built for.
    let plain = self::doc(&source, vec![add_scenery(9.0)]);
    let built = materialise(&source, &plain, Some(SLAYER)).unwrap();
    assert_eq!(built.manifest().forge[0].game, None);
    assert_eq!(
        built.manifest().map_id(),
        MapId::forge(plain.base.sha256, plain.id().unwrap())
    );
    assert_eq!(
        build(&plain, |name| (name == "fixture").then(|| source.clone()))
            .unwrap()
            .manifest(),
        built.manifest()
    );
}

#[test]
fn a_document_with_object_game_types_needs_an_engine() {
    let source = source();
    let doc = by_game_type(&source);
    for engine in [None, Some(0), Some(5), Some(-1)] {
        match materialise(&source, &doc, engine) {
            Err(Error::Game(_)) => {}
            Err(e) => panic!("{engine:?}: {e}"),
            Ok(_) => panic!("{engine:?} built a map"),
        }
    }
    assert!(matches!(map_id(&doc, None), Err(Error::Game(_))));
}

#[test]
fn a_cleared_set_puts_the_object_in_every_game_type() {
    let source = source();
    let own = own_scenery(&source);
    let doc = doc(
        &source,
        vec![games(own, Some(GameSet::of(&[CTF]))), games(own, None)],
    );
    let built = materialise(&source, &doc, Some(SLAYER)).unwrap();
    assert_eq!(scenery(&built).len(), 1);
    assert_eq!(built.manifest().forge[0].game, None);
}

#[test]
fn two_builds_for_one_engine_are_byte_equal() {
    let source = source();
    let doc = by_game_type(&source);
    let first = materialise(&source, &doc, Some(SLAYER)).unwrap();
    let second = materialise(&source, &doc, Some(SLAYER)).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());
}
