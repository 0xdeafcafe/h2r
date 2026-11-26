//! F6 acceptance on the invented scenario-edit fixture: a linked pair
//! writes two flags with one identifier, and unlinking restores the bytes.
use std::io::Cursor;

use content::Sha256;
use forge_build::{Built, Error, VERSIONS, build};
use forge_doc::{
    BlockKind, Edit, Endpoint, F32, FORMAT, FieldWrite, Forge, ForgeDoc, Link, MapRef, Op, OpId,
    Placement, Refusal, Value,
};
use map::{Map, scenario_layout};
use scenario_edit::{MapSource, Scenario, fixture};

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn source() -> MapSource {
    from(fixture::fixture_at_end())
}

fn from(bytes: Vec<u8>) -> MapSource {
    let mut source = MapSource::file("invented/fixture.map");
    source.bytes = Some(bytes.into());
    source
}

/// Two invented device groups at a free scenario offset, so a new group's
/// index differs from a new machine's zeroed group fields.
const GROUPS_AT: usize = 0x900;

fn with_device_groups() -> Vec<u8> {
    let mut bytes = fixture::fixture_at_end();
    fixture::block(&mut bytes, 0xa0, 2, GROUPS_AT);
    for (i, name) in [&b"lamps"[..], b"gates"].into_iter().enumerate() {
        let at = fixture::sfile(GROUPS_AT + i * 0x28);
        bytes[at..at + name.len()].copy_from_slice(name);
        fixture::putf(&mut bytes, at + 0x20, 1.0);
    }
    bytes
}

fn doc(source: &MapSource, ops: Vec<Op>) -> ForgeDoc {
    ForgeDoc {
        format: FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(source.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Linked map".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops,
        expect: None,
        waves: None,
    }
}

fn materialise(source: &MapSource, ops: Vec<Op>) -> Result<Built, Error> {
    build(&doc(source, ops), |name| {
        (name == "fixture").then(|| source.clone())
    })
}

fn add(id: u32, block: BlockKind, fields: Vec<FieldWrite>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block,
            like: None,
            fields,
        }),
    }
}

/// A new teleporter flag of `kind` at an invented position.
fn flag(id: u32, kind: i64, x: f32) -> Op {
    let f = |v| F32::new(v).unwrap();
    add(
        id,
        FLAGS,
        vec![
            FieldWrite {
                field: "type".into(),
                value: Value::Int(kind),
            },
            FieldWrite {
                field: "position".into(),
                value: Value::Vec3([f(x), f(0.0), f(1.0)]),
            },
        ],
    )
}

fn forge(id: u32, edit: Forge) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(edit),
    }
}

fn teleporter(id: u32, sender: Endpoint, receiver: Endpoint, channel: u16) -> Op {
    forge(
        id,
        Forge::Link(Link::Teleporter {
            sender,
            receiver,
            channel,
        }),
    )
}

fn added(id: u32) -> Endpoint {
    Endpoint::Added(OpId(id))
}

fn scenario(built: &Built) -> (Map<Cursor<&[u8]>>, Scenario) {
    let mut map = Map::from_reader(Cursor::new(built.map_bytes())).unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    (map, scenario)
}

fn int(s: &Scenario, block: BlockKind, index: usize, field: &str) -> Option<i64> {
    s.get(block, index, field).unwrap().as_int()
}

#[test]
fn a_linked_pair_writes_two_flags_with_one_identifier() {
    let source = source();
    let base = Scenario::read(&mut source.open().unwrap()).unwrap();
    let built = materialise(
        &source,
        vec![
            flag(1, 7, 2.0),
            flag(2, 8, 12.0),
            teleporter(3, added(1), added(2), 5),
        ],
    )
    .unwrap();
    let (_, after) = scenario(&built);
    // The fixture has one flag of its own; the pair follows it.
    let flags = after.block(FLAGS);
    assert_eq!(flags.len(), 3);
    assert_eq!(flags.entry(0), base.block(FLAGS).entry(0));
    for (index, kind) in [(1, 7), (2, 8)] {
        assert_eq!(int(&after, FLAGS, index, "type"), Some(kind));
        assert_eq!(int(&after, FLAGS, index, "identifier"), Some(5));
    }
    // The bots' teleporter reader (bots::autonav::traversals) takes the
    // pair from these bytes: block `scnr+0x118`, stride 0x20, type at
    // +0x10 (7 sends, 8 receives), identifier at +0x14.
    let def = scenario_layout::block(FLAGS);
    assert_eq!((def.offset, def.stride), (0x118, 0x20));
    let (sender, receiver) = (flags.entry(1).unwrap(), flags.entry(2).unwrap());
    assert_eq!(sender[0x10..0x12], 7u16.to_le_bytes());
    assert_eq!(receiver[0x10..0x12], 8u16.to_le_bytes());
    assert_eq!(sender[0x14..0x16], receiver[0x14..0x16]);
    assert_eq!(sender[0x14..0x16], 5i16.to_le_bytes());
}

#[test]
fn unlink_restores_the_bytes() {
    let source = source();
    let pair = || vec![flag(1, 7, 2.0), flag(2, 8, 12.0)];
    let plain = materialise(&source, pair()).unwrap();
    let mut ops = pair();
    ops.push(teleporter(3, added(1), added(2), 5));
    let linked = materialise(&source, ops.clone()).unwrap();
    assert_ne!(linked.map_bytes(), plain.map_bytes());
    ops.push(forge(4, Forge::Unlink { link: OpId(3) }));
    let unlinked = materialise(&source, ops).unwrap();
    assert_eq!(unlinked.map_bytes(), plain.map_bytes());
}

#[test]
fn a_link_from_a_flag_of_another_type_is_refused_by_name() {
    let source = source();
    // The fixture's own flag is not a teleporter.
    let base = Endpoint::base(FLAGS, 0);
    let error = materialise(
        &source,
        vec![flag(1, 8, 2.0), teleporter(2, base, added(1), 5)],
    )
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::Document(Refusal::Invalid { reason, .. })
            if reason == "flag 0 is not a teleporter sender"),
        "{error}"
    );
    // A receiver linked as a sender is refused the same way.
    let error = materialise(
        &source,
        vec![
            flag(1, 8, 2.0),
            flag(2, 8, 4.0),
            teleporter(3, added(1), added(2), 5),
        ],
    )
    .err()
    .unwrap();
    assert!(
        error.to_string().contains("not a teleporter sender"),
        "{error}"
    );
}

#[test]
fn device_links_add_one_group_their_machines_name() {
    let original = with_device_groups();
    let source = from(original.clone());
    let machines = || {
        vec![
            add(1, BlockKind::Machines, vec![]),
            add(2, BlockKind::Machines, vec![]),
            add(3, BlockKind::Machines, vec![]),
        ]
    };
    let plain = materialise(&source, machines()).unwrap();
    let mut ops = machines();
    ops.push(forge(
        4,
        Forge::Link(Link::Device {
            from: added(1),
            to: added(3),
        }),
    ));
    let built = materialise(&source, ops.clone()).unwrap();
    let (mut map, after) = scenario(&built);
    let id = map.scenario().unwrap().id;
    let root = map.read_tag_bytes(id).unwrap();
    let word = |at: usize| u32::from_le_bytes(root[at..at + 4].try_into().unwrap());
    // The map's two groups stay first and keep their bytes; the link adds
    // the third.
    assert_eq!(word(0xa0), 3);
    let at = map.file_offset(word(0xa4), 3 * 0x28).unwrap();
    let groups = map.read_file_range(at, 3 * 0x28).unwrap();
    let mine = fixture::sfile(GROUPS_AT);
    assert_eq!(groups[..0x50], original[mine..mine + 0x50]);
    assert!(groups[0x50..].starts_with(b"forge link 1\0"));
    assert_eq!(groups[0x70..], [0; 8]);
    let position_group = |s: &Scenario, i: usize| {
        let entry = s.block(BlockKind::Machines).entry(i).unwrap();
        i16::from_le_bytes(entry[0x36..0x38].try_into().unwrap())
    };
    let (_, unlinked) = scenario(&plain);
    assert_eq!(position_group(&unlinked, 0), 0);
    assert_eq!(position_group(&after, 0), 2);
    assert_eq!(position_group(&after, 1), position_group(&unlinked, 1));
    assert_eq!(position_group(&after, 2), 2);

    ops.push(forge(5, Forge::Unlink { link: OpId(4) }));
    assert_eq!(
        materialise(&source, ops).unwrap().map_bytes(),
        plain.map_bytes()
    );
}

#[test]
fn removing_a_linked_flag_is_refused_before_materialising() {
    let source = source();
    let error = materialise(
        &source,
        vec![
            flag(1, 7, 2.0),
            flag(2, 8, 12.0),
            teleporter(3, added(1), added(2), 5),
            Op {
                id: OpId(4),
                edit: Edit::Placement(Placement::Remove {
                    block: FLAGS,
                    index: 2,
                }),
            },
        ],
    )
    .err()
    .unwrap();
    assert!(
        error.to_string().contains("remove the entry's links first"),
        "{error}"
    );
}

/// The generator half of F6: the bots' own flag reader pairs the flags a
/// link wrote, and linking them into a mesh makes the teleporter edge.
#[test]
fn the_bots_generator_reads_a_linked_pair_and_links_it() {
    use bots::autonav::{Teleporter, link_teleporters, teleporter_flags};
    use bots::cache::MapData;
    use bots::nav::{Kind, NavMesh, Poly};

    let source = source();
    let built = materialise(
        &source,
        vec![
            flag(1, 7, 2.0),
            flag(2, 8, 12.0),
            teleporter(3, added(1), added(2), 5),
        ],
    )
    .unwrap();
    let (mut map, _) = scenario(&built);
    let flags = teleporter_flags(&mut map).unwrap();
    let ours: Vec<&Teleporter> = flags.iter().filter(|f| f.channel == 5).collect();
    assert_eq!(ours.len(), 2, "{flags:?}");
    let sender = ours.iter().find(|f| f.sender).expect("a sender");
    let receiver = ours.iter().find(|f| !f.sender).expect("a receiver");
    assert_eq!(sender.position, [2.0, 0.0, 1.0]);
    assert_eq!(receiver.position, [12.0, 0.0, 1.0]);

    // Two floors under the ends: the generator links the pair.
    let floor = |x: f32| {
        Poly::new(vec![
            [x - 2.0, -2.0, 0.5],
            [x + 2.0, -2.0, 0.5],
            [x + 2.0, 2.0, 0.5],
            [x - 2.0, 2.0, 0.5],
        ])
    };
    let mut data = MapData {
        nav: NavMesh::new(vec![floor(2.0), floor(12.0)], vec![]),
        ..Default::default()
    };
    link_teleporters(&mut data, &flags);
    let edges: Vec<_> = data
        .nav
        .edges
        .iter()
        .filter(|e| e.kind == Kind::Teleporter)
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(edges, [(0, 1)]);
}
