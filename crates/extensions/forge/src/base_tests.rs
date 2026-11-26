//! The session's things over a fake world and an invented map: two weapons,
//! a starting point, a teleporter and a goal of the map's own, and a
//! palette with a crate and a rifle. No map or game installation.
use std::collections::BTreeMap;

use forge_doc::{
    BuilderVersions, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, Limits, MapRef, Op,
    Sha256, TagKey, Value, WorldBounds,
};

use crate::{BaseRow, BaseRows, BlockKind, Frame, PaletteItem, Session, ThingKind, World};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    Place(u32, Frame),
    Move(u32, Frame),
    Delete(u32),
    Respawn(u32, Frame),
    Channel(u32, i16),
    BaseMove(BlockKind, u32, Frame),
    BaseField(BlockKind, u32, String, Value),
    BaseInert(BlockKind, u32),
    BaseRestore(BlockKind, u32, Vec<u8>),
    KitAdd(BlockKind, u32, Vec<(String, Value)>),
    KitDrop(BlockKind, u32),
}

/// A world that records each call. `refuse_at` refuses the call at that
/// position, once.
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) next: u32,
    pub calls: Vec<Call>,
    pub refuse_at: Option<usize>,
}

impl Fake {
    fn call(&mut self, call: Call) -> Result<(), String> {
        if self.refuse_at == Some(self.calls.len()) {
            self.refuse_at = None;
            return Err("named engine refusal".into());
        }
        self.calls.push(call);
        Ok(())
    }
}

impl World for Fake {
    fn place(&mut self, _: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.call(Call::Place(self.next + 1, frame))?;
        self.next += 1;
        Ok(self.next)
    }
    fn respawn(&mut self, _: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.call(Call::Respawn(self.next + 1, frame))?;
        self.next += 1;
        Ok(self.next)
    }
    fn move_object(&mut self, handle: u32, frame: Frame) -> Result<(), String> {
        self.call(Call::Move(handle, frame))
    }
    fn delete(&mut self, handle: u32) -> Result<(), String> {
        self.call(Call::Delete(handle))
    }
    fn channel(&mut self, flag: u32, channel: i16) -> Result<(), String> {
        self.call(Call::Channel(flag, channel))
    }
    fn base_move(&mut self, block: BlockKind, index: u32, frame: Frame) -> Result<(), String> {
        self.call(Call::BaseMove(block, index, frame))
    }
    fn base_field(
        &mut self,
        block: BlockKind,
        index: u32,
        field: &str,
        value: &Value,
    ) -> Result<(), String> {
        self.call(Call::BaseField(block, index, field.into(), value.clone()))
    }
    fn base_inert(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
        self.call(Call::BaseInert(block, index))
    }
    fn base_restore(&mut self, block: BlockKind, index: u32, row: &[u8]) -> Result<(), String> {
        self.call(Call::BaseRestore(block, index, row.to_vec()))
    }
    fn kit_add(
        &mut self,
        block: BlockKind,
        index: u32,
        fields: &[(&'static str, Value)],
    ) -> Result<(), String> {
        let fields = fields.iter().map(|(n, v)| (n.to_string(), v.clone())).collect();
        self.call(Call::KitAdd(block, index, fields))
    }
    fn kit_drop(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
        self.call(Call::KitDrop(block, index))
    }
}

pub(crate) fn float(v: f32) -> Value {
    Value::Float(F32::new(v).unwrap())
}

pub(crate) fn vec3(v: [f32; 3]) -> Value {
    Value::Vec3(v.map(|c| F32::new(c).unwrap()))
}

pub(crate) fn frame(x: f32, yaw: f32) -> Frame {
    Frame {
        position: [x, 1., 2.],
        rotation: [yaw, 0., 0.],
    }
}

pub(crate) fn tag(group: &str, path: &str) -> TagKey {
    TagKey {
        group: group.into(),
        path: path.into(),
    }
}

pub(crate) fn crate_tag() -> TagKey {
    tag("bloc", r"fixture\cube")
}

pub(crate) fn rifle() -> TagKey {
    tag("weap", r"fixture\rifle")
}

/// A row of the map with these fields: its bytes are its index repeated,
/// so a restore's row is told apart from another's.
fn row(block: BlockKind, index: u32, fields: Vec<Value>) -> BaseRow {
    let stride = map::scenario_layout::block(block).stride as usize;
    BaseRow {
        block,
        index,
        raw: vec![index as u8 + 1; stride],
        print: forge_doc::print(block, &fields).unwrap(),
        fields,
    }
}

/// The invented map's rows: weapons 0 and 1, starting point 0, the netgame
/// flags 0 (a teleporter sender on channel 1), 1 (a CTF flag spawn) and 2
/// (a teleporter receiver on channel 3), and item spawner 0.
pub(crate) fn rows() -> Vec<BaseRow> {
    let weapon = |index: u32, x: f32| {
        row(
            BlockKind::Weapons,
            index,
            vec![
                Value::Tag(Some(rifle())),
                Value::Int(-1),
                Value::Int(0),
                vec3([x, 0., 0.]),
                vec3([0., 0., 0.]),
                float(0.),
            ],
        )
    };
    let flag = |index: u32, ty: i64, team: i64, identifier: i64| {
        row(
            BlockKind::NetgameFlags,
            index,
            vec![
                vec3([index as f32, 5., 0.]),
                float(0.),
                Value::Int(ty),
                Value::Int(team),
                Value::Int(identifier),
            ],
        )
    };
    let mut spawner = vec![Value::Int(0)];
    spawner.extend([12, 12, 12, 12, 0, 30, 0].map(Value::Int));
    spawner.extend([
        Value::Int(0),
        vec3([0., 6., 0.]),
        vec3([0., 0., 0.]),
        Value::Tag(Some(tag("itmc", r"fixture\rifles"))),
    ]);
    vec![
        weapon(0, -3.),
        weapon(1, 4.),
        row(
            BlockKind::StartingLocations,
            0,
            vec![
                vec3([0., -4., 0.]),
                float(1.5),
                Value::Int(0),
                Value::Int(-1),
            ],
        ),
        flag(0, 7, 8, 1),
        flag(1, 0, 1, 0),
        flag(2, 8, 8, 3),
        row(BlockKind::NetgameEquipment, 0, spawner),
    ]
}

fn draft() -> ForgeDoc {
    ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "ownroom".into(),
            sha256: Sha256::of(b"an invented room with its own things"),
        },
        sources: vec![],
        title: "Own room".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    }
}

/// A session over `rows`, opened as the host opens one.
pub(crate) fn open(rows: Vec<BaseRow>) -> Session {
    open_with(BaseRows {
        rows,
        left_out: vec![],
    })
}

fn open_with(base: BaseRows) -> Session {
    let limits = Limits {
        base_counts: BTreeMap::from([
            (BlockKind::Weapons, 2),
            (BlockKind::Crates, 0),
            (BlockKind::StartingLocations, 1),
            (BlockKind::NetgameFlags, 3),
            (BlockKind::NetgameEquipment, 1),
        ]),
        world: Some(WorldBounds {
            min: [-20.; 3],
            max: [20.; 3],
        }),
        ..Default::default()
    };
    let palette = vec![
        PaletteItem {
            block: BlockKind::Crates,
            tag: crate_tag(),
        },
        PaletteItem {
            block: BlockKind::Weapons,
            tag: rifle(),
        },
    ];
    Session::new(draft(), limits, palette, 4)
        .unwrap()
        .with_base(base)
}

pub(crate) fn weapon(index: u32) -> Endpoint {
    Endpoint::base(BlockKind::Weapons, index)
}

/// The entry op of each of the document's ops.
pub(crate) fn entry_ops(s: &Session) -> Vec<EntryOp> {
    s.document()
        .ops
        .iter()
        .map(|op| match &op.edit {
            Edit::Entry(edit) => edit.clone(),
            edit => panic!("expected an entry op, not {edit:?}"),
        })
        .collect()
}

#[test]
fn things_list_the_maps_own_entries_then_the_added_ones() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let added = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let things = s.things();
    let kinds: Vec<_> = things.iter().map(|t| (t.block, t.kind, t.own)).collect();
    assert_eq!(
        kinds,
        [
            (BlockKind::Weapons, ThingKind::Object, true),
            (BlockKind::Weapons, ThingKind::Object, true),
            (BlockKind::StartingLocations, ThingKind::StartingPoint, true),
            (BlockKind::NetgameFlags, ThingKind::Teleporter, true),
            (BlockKind::NetgameFlags, ThingKind::Goal, true),
            (BlockKind::NetgameFlags, ThingKind::Teleporter, true),
            (BlockKind::NetgameEquipment, ThingKind::ItemSpawner, true),
            (BlockKind::Crates, ThingKind::Object, false),
        ]
    );
    // An own entry is named with its print and the layout's fields.
    let rows = rows();
    assert_eq!(
        things[1].id,
        Endpoint::Base {
            block: BlockKind::Weapons,
            index: 1,
            print: Some(rows[1].print),
        }
    );
    assert_eq!(things[1].fields[3], ("position", vec3([4., 0., 0.])));
    assert_eq!(
        things[1].frame,
        Frame {
            position: [4., 0., 0.],
            rotation: [0.; 3],
        }
    );
    assert_eq!(things[2].frame.rotation, [1.5, 0., 0.]);
    assert!(!things[1].changed);
    assert_eq!(things[7].id, Endpoint::Added(added));
    assert!(things[7].changed);
}

#[test]
fn a_drop_of_an_own_entry_is_two_sets_in_one_undo_step_and_one_base_move() {
    let mut s = open(rows());
    let start = s.document().clone();
    let mut w = Fake::default();
    let first = s.thing(weapon(1)).unwrap().frame;
    let to = frame(3., 0.5);
    s.move_thing(&mut w, 1, weapon(1), to).unwrap();

    let target = Endpoint::Base {
        block: BlockKind::Weapons,
        index: 1,
        print: Some(rows()[1].print),
    };
    assert_eq!(
        entry_ops(&s),
        [
            EntryOp::Set {
                target,
                write: FieldWrite {
                    field: "position".into(),
                    value: vec3(to.position),
                },
            },
            EntryOp::Set {
                target,
                write: FieldWrite {
                    field: "rotation".into(),
                    value: vec3(to.rotation),
                },
            },
        ]
    );
    assert_eq!(w.calls, [Call::BaseMove(BlockKind::Weapons, 1, to)]);
    let moved = s.thing(weapon(1)).unwrap();
    assert_eq!((moved.frame, moved.changed), (to, true));
    assert_eq!(forge_doc::fold(&start, s.log()).unwrap(), *s.document());

    // One undo takes both sets back with one move to the first frame.
    s.undo(&mut w, 2).unwrap();
    assert!(s.document().ops.is_empty());
    assert_eq!(w.calls[1..], [Call::BaseMove(BlockKind::Weapons, 1, first)]);
    assert!(!s.thing(weapon(1)).unwrap().changed);
    s.redo(&mut w, 3).unwrap();
    assert_eq!(w.calls[2..], [Call::BaseMove(BlockKind::Weapons, 1, to)]);
}

#[test]
fn a_facing_entry_moves_by_its_position_and_facing() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let start = Endpoint::base(BlockKind::StartingLocations, 0);
    let to = frame(2., -0.25);
    s.move_thing(&mut w, 1, start, to).unwrap();
    let fields: Vec<_> = entry_ops(&s)
        .into_iter()
        .map(|op| match op {
            EntryOp::Set { write, .. } => (write.field, write.value),
            op => panic!("expected a set, not {op:?}"),
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("position".to_string(), vec3(to.position)),
            ("facing".to_string(), float(-0.25)),
        ]
    );
    assert_eq!(
        w.calls,
        [Call::BaseMove(BlockKind::StartingLocations, 0, to)]
    );
    // Turning only writes the facing.
    s.move_thing(&mut w, 2, start, frame(2., 0.75)).unwrap();
    assert_eq!(s.document().ops.len(), 3);
}

#[test]
fn a_print_the_fake_changes_is_refused_and_the_document_keeps_its_bytes() {
    let mut a = open(rows());
    let mut w = Fake::default();
    a.move_thing(&mut w, 1, weapon(1), frame(3., 0.)).unwrap();
    let op: Op = a.document().ops[0].clone();

    // Another tool wrote weapon 1's scale in the scenario this session
    // opened on, so its row is not the one the edit was made on.
    let mut changed = rows();
    changed[1].fields[5] = float(2.);
    changed[1].print = forge_doc::print(BlockKind::Weapons, &changed[1].fields).unwrap();
    let mut b = open(changed);
    let bytes = b.document().canonical_json().unwrap();
    let head = b.head();
    let mut other = Fake::default();
    let error = b.propose(&mut other, 1, op, None).unwrap_err();
    assert!(
        error.contains("weapons entry 1 is not the one this edit was made on"),
        "{error}"
    );
    assert_eq!(b.document().canonical_json().unwrap(), bytes);
    assert_eq!(b.head(), head);
    assert!(b.log().is_empty());
    assert!(other.calls.is_empty());
}

#[test]
fn a_block_that_did_not_read_has_no_things_and_refuses_edits() {
    let rows: Vec<_> = rows()
        .into_iter()
        .filter(|r| r.block != BlockKind::NetgameFlags)
        .collect();
    let mut s = open_with(BaseRows {
        rows,
        left_out: vec!["netgame_flags: guest memory at 0x10 does not read".into()],
    });
    assert_eq!(
        s.left_out(),
        ["netgame_flags: guest memory at 0x10 does not read"]
    );
    assert!(
        s.things()
            .iter()
            .all(|t| t.block != BlockKind::NetgameFlags)
    );
    let mut w = Fake::default();
    let flag = Endpoint::base(BlockKind::NetgameFlags, 0);
    let error = s.move_thing(&mut w, 1, flag, frame(1., 0.)).unwrap_err();
    assert!(error.contains("Aim at a thing"), "{error}");
    // A peer's edit of it cannot be checked, so it is refused.
    let op = Op {
        id: forge_doc::OpId(1),
        edit: Edit::Entry(EntryOp::Set {
            target: Endpoint::Base {
                block: BlockKind::NetgameFlags,
                index: 0,
                print: Some(self::rows()[3].print),
            },
            write: FieldWrite {
                field: "team".into(),
                value: Value::Int(2),
            },
        }),
    };
    let error = s.propose(&mut w, 1, op, None).unwrap_err();
    assert!(error.contains("did not read"), "{error}");
    assert!(s.document().ops.is_empty());
    assert!(w.calls.is_empty());

    // A block whose rows are not the map's count is left out by name.
    let mut short = rows_of(BlockKind::Weapons);
    short.pop();
    let s = open(short);
    assert_eq!(
        s.left_out(),
        ["weapons: the loaded scenario has 1 rows, the map 2"]
    );
    assert!(s.things().is_empty());
}

fn rows_of(block: BlockKind) -> Vec<BaseRow> {
    rows().into_iter().filter(|r| r.block == block).collect()
}

#[test]
fn moving_an_added_entry_writes_entry_ops_by_its_add() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let id = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let to = frame(6., 0.25);
    s.move_thing(&mut w, 2, Endpoint::Added(id), to).unwrap();
    let ops = &s.document().ops;
    assert_eq!(ops.len(), 3);
    for op in &ops[1..] {
        assert!(matches!(
            &op.edit,
            Edit::Entry(EntryOp::Set {
                target: Endpoint::Added(add),
                ..
            }) if *add == id
        ));
    }
    assert_eq!(w.calls, [Call::Place(1, frame(1., 0.)), Call::Move(1, to)]);
    assert_eq!(s.objects()[0].frame, to);
    assert_eq!(s.thing(Endpoint::Added(id)).unwrap().frame, to);
    s.undo(&mut w, 3).unwrap();
    assert_eq!(w.calls[2..], [Call::Move(1, frame(1., 0.))]);
    assert_eq!(s.document().ops.len(), 1);
}

#[test]
fn a_hold_moves_at_most_once_a_tick_and_the_drop_is_one_undo_step() {
    let mut s = open(rows());
    let mut w = Fake::default();
    let first = s.thing(weapon(0)).unwrap().frame;
    let (a, b) = (frame(1., 0.), frame(2., 0.3));
    s.hold(&mut w, 10, weapon(0), a).unwrap();
    // A second move in the same tick is dropped.
    s.hold(&mut w, 10, weapon(0), b).unwrap();
    s.hold(&mut w, 11, weapon(0), b).unwrap();
    assert_eq!(
        w.calls,
        [
            Call::BaseMove(BlockKind::Weapons, 0, a),
            Call::BaseMove(BlockKind::Weapons, 0, b),
        ]
    );
    // The hold changes nothing but the world.
    assert!(s.document().ops.is_empty());
    assert!(s.log().is_empty());
    assert_eq!(s.thing(weapon(0)).unwrap().frame, first);

    // The drop where the hold left it needs no further move.
    s.move_thing(&mut w, 12, weapon(0), b).unwrap();
    assert_eq!(s.document().ops.len(), 2);
    assert_eq!(w.calls.len(), 2);
    s.undo(&mut w, 13).unwrap();
    assert!(s.document().ops.is_empty());
    assert_eq!(w.calls[2..], [Call::BaseMove(BlockKind::Weapons, 0, first)]);

    // A hold let go puts the entry back where the document has it.
    s.hold(&mut w, 14, weapon(0), a).unwrap();
    s.release(&mut w).unwrap();
    assert_eq!(
        w.calls[3..],
        [
            Call::BaseMove(BlockKind::Weapons, 0, a),
            Call::BaseMove(BlockKind::Weapons, 0, first),
        ]
    );
}

#[test]
fn a_refused_base_move_leaves_the_document_log_and_world_alone() {
    let mut s = open(rows());
    let doc = s.document().clone();
    let mut w = Fake {
        refuse_at: Some(0),
        ..Default::default()
    };
    let error = s
        .move_thing(&mut w, 1, weapon(1), frame(3., 0.))
        .unwrap_err();
    assert_eq!(error, "named engine refusal");
    assert_eq!(s.document(), &doc);
    assert!(s.log().is_empty());
    assert!(w.calls.is_empty());
    assert!(!s.thing(weapon(1)).unwrap().changed);
    // Out of the world's bounds is refused before the world is asked.
    assert!(s.move_thing(&mut w, 2, weapon(1), frame(30., 0.)).is_err());
    assert!(w.calls.is_empty());
}

#[test]
fn setting_an_area_is_one_undoable_document_edit() {
    let mut session = open(rows());
    let mut world = Fake::default();
    let goal = Endpoint::base(BlockKind::NetgameFlags, 1);
    let area = forge_doc::MarkerArea::radius(3.0);
    session
        .set_area(&mut world, 1, goal, Some(area.clone()))
        .unwrap();
    let read = |session: &Session| {
        session
            .areas()
            .unwrap()
            .get(&goal)
            .map(forge_doc::Area::marker_area)
    };
    assert_eq!(read(&session), Some(area.clone()));
    assert_eq!(session.log().len(), 1);
    session.undo(&mut world, 2).unwrap();
    assert_eq!(read(&session), None);
    session.redo(&mut world, 3).unwrap();
    assert_eq!(read(&session), Some(area.clone()));
    session.reset(&mut world, 4, goal).unwrap();
    assert_eq!(read(&session), None);
    session.undo(&mut world, 5).unwrap();
    assert_eq!(read(&session), Some(area));
    session.set_area(&mut world, 6, goal, None).unwrap();
    assert_eq!(read(&session), None);
    assert!(session.set_area(&mut world, 7, goal, None).is_err());
    assert_eq!(session.log().len(), 6);
    assert!(world.calls.is_empty());
}
