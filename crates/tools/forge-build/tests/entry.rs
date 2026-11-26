//! M3 acceptance on the invented scenario-edit fixture: entry ops
//! materialise through their running indices, and every base entry they
//! name is checked by print first (docs/game/forge-map-variants.md).
use std::io::Cursor;

use content::Sha256;
use forge_build::{Built, Error, VERSIONS, base_print, build};
use forge_doc::{
    BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, MapRef, Op, OpId, Print,
    Printed, TagKey, Value, print,
};
use map::{Map, TagId, scenario_layout};
use scenario_edit::{
    Command, MapSource, Scenario, Session, Value as ScenarioValue, fixture, layout::Role, save,
};

const WEAPONS: BlockKind = BlockKind::Weapons;
const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn source() -> MapSource {
    let mut source = MapSource::file("invented/fixture.map");
    source.bytes = Some(fixture::fixture().into());
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

fn materialise(source: &MapSource, edits: Vec<Edit>) -> Result<Built, Error> {
    let doc = doc(source, edits);
    build(&doc, |name| (name == "fixture").then(|| source.clone()))
}

fn f(x: f32) -> F32 {
    F32::new(x).unwrap()
}

/// The fixture's typed fields of one entry, read through the editor's
/// own decoder rather than the materialiser's.
fn values(source: &MapSource, block: BlockKind, index: usize) -> Vec<Value> {
    let mut map = source.open().unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    let key = |id: TagId| {
        let tag = map.tag(id).unwrap();
        Some(TagKey {
            group: String::from_utf8_lossy(&tag.group.0).into_owned(),
            path: tag.name.clone().unwrap(),
        })
    };
    scenario_layout::block(block)
        .fields
        .iter()
        .map(
            |field| match scenario.get(block, index, field.name).unwrap() {
                ScenarioValue::Int(-1) if field.role == Role::PaletteIndex => Value::Tag(None),
                ScenarioValue::Int(n) if field.role == Role::PaletteIndex => {
                    let reference = scenario.block(block).palette[n as usize];
                    Value::Tag(if reference.is_none() {
                        None
                    } else {
                        key(reference.id)
                    })
                }
                ScenarioValue::Int(n) => Value::Int(n),
                ScenarioValue::Float(x) => Value::Float(f(x)),
                ScenarioValue::Vec3(v) => Value::Vec3(v.map(f)),
                ScenarioValue::Tag(r) if r.is_none() => Value::Tag(None),
                ScenarioValue::Tag(r) => Value::Tag(key(r.id)),
            },
        )
        .collect()
}

/// The fixture's entry, named with its print.
fn own(source: &MapSource, block: BlockKind, index: u32) -> Endpoint {
    Endpoint::Base {
        block,
        index,
        print: Some(print(block, &values(source, block, index as usize)).unwrap()),
    }
}

fn set(target: Endpoint, field: &str, value: Value) -> Edit {
    Edit::Entry(EntryOp::Set {
        target,
        write: FieldWrite {
            field: field.into(),
            value,
        },
    })
}

fn remove(target: Endpoint) -> Edit {
    Edit::Entry(EntryOp::Remove { target })
}

#[test]
fn the_materialisers_print_is_the_print_of_the_editors_fields() {
    let source = source();
    let mut map = source.open().unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    let mut compared = 0;
    for def in scenario_layout::BLOCKS {
        for index in 0..scenario.block(def.kind).len() {
            assert_eq!(
                base_print(&map, &scenario, def.kind, index as u32).unwrap(),
                print(def.kind, &values(&source, def.kind, index)).unwrap(),
                "{} {index}",
                def.kind.name()
            );
            compared += 1;
        }
    }
    // Weapons 2, starting locations 2, a flag, a spawner and a scenery.
    assert_eq!(compared, 7);
}

#[test]
fn moving_an_own_entry_changes_its_bytes_and_the_checksum_only() {
    let source = source();
    let original = source.bytes.as_deref().unwrap().to_vec();
    let built = materialise(
        &source,
        vec![set(
            own(&source, WEAPONS, 1),
            "position",
            Value::Vec3([f(7.0), f(8.0), f(9.0)]),
        )],
    )
    .unwrap();
    let output = built.map_bytes();
    assert_eq!(output.len(), original.len());

    let position = fixture::sfile(fixture::WEAPONS_AT + 0x54 + 8);
    let allowed = [position..position + 12, 0x2d0..0x2d4];
    let mut changed = 0;
    for (index, (a, b)) in original.iter().zip(output).enumerate() {
        if a != b {
            assert!(
                allowed.iter().any(|r| r.contains(&index)),
                "unexpected write at {index:#x}"
            );
            changed += 1;
        }
    }
    assert!(changed > 0);

    let mut map = Map::from_reader(Cursor::new(output)).unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    assert_eq!(
        scenario.get(WEAPONS, 1, "position").unwrap().as_vec3(),
        Some([7.0, 8.0, 9.0])
    );
    assert_eq!(
        built.prints,
        vec![Printed {
            block: WEAPONS,
            index: 1,
            print: own(&source, WEAPONS, 1).print().unwrap(),
        }]
    );
}

#[test]
fn a_removal_swap_removes_and_repoints_names_as_the_editor_does() {
    let source = source();
    let built = materialise(&source, vec![remove(own(&source, WEAPONS, 0))]).unwrap();

    let mut editor = Session::new();
    editor.build_scenes = false;
    editor.open_named(source.clone(), "fixture").unwrap();
    editor.apply(Command::remove(WEAPONS, 0)).unwrap();
    let mut expected = Cursor::new(Vec::new());
    save::write_patched(
        &mut source.open().unwrap(),
        editor.scenario().unwrap(),
        &mut expected,
    )
    .unwrap();
    assert_eq!(built.map_bytes(), expected.into_inner().as_slice());

    // Weapon 1 moved into slot 0 and its name follows it; weapon 0's name
    // names nothing.
    let mut map = Map::from_reader(Cursor::new(built.map_bytes())).unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    assert_eq!(scenario.block(WEAPONS).len(), 1);
    assert_eq!(scenario.names.target(0), Some((2, 0)));
    assert_eq!(scenario.names.target(2), Some((2, -1)));
}

#[test]
fn a_wrong_print_is_variant_print_with_its_block_and_index_and_no_file() {
    let source = source();
    let wrong = Endpoint::Base {
        block: FLAGS,
        index: 0,
        print: Some(Print([0x5a; 8])),
    };
    let edits = vec![
        set(own(&source, WEAPONS, 0), "scale", Value::Float(f(2.0))),
        set(wrong, "identifier", Value::Int(4)),
    ];
    let error = match materialise(&source, edits) {
        Err(error) => error,
        Ok(_) => panic!("a wrong print built a map"),
    };
    assert!(
        matches!(
            error,
            Error::Print {
                block: FLAGS,
                index: 0
            }
        ),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("variant_print: the map's netgame_flags entry 0"),
        "{error}"
    );
}

#[test]
fn two_builds_of_entry_ops_are_byte_equal() {
    let source = source();
    let edits = || {
        vec![
            set(
                own(&source, WEAPONS, 1),
                "rotation",
                Value::Vec3([f(0.5), f(0.0), f(0.0)]),
            ),
            remove(own(&source, WEAPONS, 0)),
            set(own(&source, FLAGS, 0), "identifier", Value::Int(4)),
        ]
    };
    let first = materialise(&source, edits()).unwrap();
    let second = materialise(&source, edits()).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());
    assert_eq!(first.prints, second.prints);
    assert_eq!(
        first
            .prints
            .iter()
            .map(|p| (p.block, p.index))
            .collect::<Vec<_>>(),
        vec![(WEAPONS, 0), (WEAPONS, 1), (FLAGS, 0)]
    );
}
