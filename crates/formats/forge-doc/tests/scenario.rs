//! F1 parity against the editor on an invented cache.
use std::io::Cursor;

use forge_doc::{
    Apply, BlockKind, BuilderVersions, Edit, F32, FORMAT, FieldWrite, ForgeDoc, Limits, MapRef, Op,
    OpId, Placement, Refusal, Sha256, TagKey, TagWrite, Value, replay,
};
use map::{FourCC, TagQuery};
use scenario_edit::{
    Command, MapSource, Session, SetField, TagRef, Value as ScenarioValue, edit,
    fixture::{fixture, id},
    layout::{self, Role},
    save,
};

fn open() -> (Session, MapSource) {
    let mut source = MapSource::file("forge-fixture/fixture.map");
    source.bytes = Some(fixture().into());
    let mut session = Session::new();
    session.build_scenes = false;
    session.open_named(source.clone(), "fixture").unwrap();
    (session, source)
}

fn editor_error(error: impl std::fmt::Display) -> Refusal {
    Refusal::Invalid {
        what: "scenario".into(),
        reason: error.to_string(),
    }
}

struct ScenarioApply {
    session: Session,
    source: MapSource,
}

impl ScenarioApply {
    fn reference(&self, tag: Option<&TagKey>) -> forge_doc::Result<TagRef> {
        let Some(key) = tag else {
            return Ok(TagRef::NONE);
        };
        let group = FourCC(key.group.as_bytes().try_into().map_err(editor_error)?);
        let map = self.source.open().map_err(editor_error)?;
        let tag = map
            .tag(TagQuery::NameInGroup(&key.path, group))
            .map_err(editor_error)?;
        Ok(TagRef {
            group: tag.group,
            id: tag.id,
        })
    }

    fn field(
        &self,
        block: BlockKind,
        index: usize,
        write: &FieldWrite,
    ) -> forge_doc::Result<SetField> {
        let def = layout::block(block)
            .field(&write.field)
            .ok_or_else(|| editor_error("unknown field"))?;
        let value = match &write.value {
            Value::Int(n) => ScenarioValue::Int(*n),
            Value::Float(n) => ScenarioValue::Float(n.get()),
            Value::Vec3(v) => ScenarioValue::Vec3(v.map(F32::get)),
            Value::Tag(tag) if def.role == Role::PaletteIndex => {
                let reference = self.reference(tag.as_ref())?;
                let index = if reference.is_none() {
                    -1
                } else {
                    self.session
                        .scenario()
                        .unwrap()
                        .block(block)
                        .palette
                        .iter()
                        .position(|entry| *entry == reference)
                        .ok_or_else(|| editor_error("tag is absent from the palette"))?
                        as i64
                };
                ScenarioValue::Int(index)
            }
            Value::Tag(tag) => ScenarioValue::Tag(self.reference(tag.as_ref())?),
            _ => return Err(editor_error("wrong value type for placement")),
        };
        Ok(SetField {
            block,
            index,
            field: def.name,
            value,
        })
    }
}

impl Apply for ScenarioApply {
    fn placement(&mut self, placement: &Placement) -> forge_doc::Result<()> {
        let block = placement.block();
        let command = match placement {
            Placement::Set { index, write, .. } => {
                let set = self.field(block, *index as usize, write)?;
                Command::set(block, set.index, set.field, set.value)
            }
            Placement::Add { like, fields, .. } => {
                let scenario = self.session.scenario().unwrap();
                let index = scenario.block(block).len();
                let bytes = scenario
                    .new_entry(block, like.map(|i| i as usize), None)
                    .map_err(editor_error)?;
                let mut command = Command::add(block, bytes);
                for write in fields {
                    command
                        .ops
                        .push(edit::Op::Set(self.field(block, index, write)?));
                }
                command
            }
            Placement::Remove { index, .. } => Command::remove(block, *index as usize),
        };
        self.session.apply(command).map_err(editor_error)
    }

    fn tag_field(&mut self, _: &TagWrite) -> forge_doc::Result<()> {
        Err(editor_error("this adapter edits scenario placements"))
    }
}

fn output(source: &MapSource, session: &Session) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    save::write_patched(
        &mut source.open().unwrap(),
        session.scenario().unwrap(),
        &mut out,
    )
    .unwrap();
    out.into_inner()
}

fn draft(source: &MapSource, edits: Vec<Placement>) -> ForgeDoc {
    ForgeDoc {
        format: FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(source.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "An invented Forge map".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: edits
            .into_iter()
            .enumerate()
            .map(|(i, edit)| Op {
                id: OpId(i as u32 + 1),
                edit: Edit::Placement(edit),
            })
            .collect(),
        expect: None,
    }
}

#[test]
fn placement_replay_matches_the_sessions_patched_bytes() {
    let (session, source) = open();
    let (mut editor, _) = open();
    let mut target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: layout::BLOCKS
            .iter()
            .map(|def| {
                (
                    def.kind,
                    target.session.scenario().unwrap().block(def.kind).len() as u32,
                )
            })
            .collect(),
        ..Limits::default()
    };

    let block = BlockKind::Weapons;
    let weapon = TagKey {
        group: "weap".into(),
        path: "objects\\test\\blaster".into(),
    };
    let position = [3.0, 4.0, 5.0];
    let doc = draft(
        &target.source,
        vec![
            Placement::Set {
                block,
                index: 0,
                write: FieldWrite {
                    field: "scale".into(),
                    value: Value::Float(F32::new(2.0).unwrap()),
                },
            },
            Placement::Add {
                block,
                like: Some(1),
                fields: vec![
                    FieldWrite {
                        field: "position".into(),
                        value: Value::Vec3(position.map(|v| F32::new(v).unwrap())),
                    },
                    FieldWrite {
                        field: "palette index".into(),
                        value: Value::Tag(Some(weapon.clone())),
                    },
                ],
            },
            Placement::Remove { block, index: 1 },
            Placement::Set {
                block: BlockKind::NetgameEquipment,
                index: 0,
                write: FieldWrite {
                    field: "item".into(),
                    value: Value::Tag(Some(weapon)),
                },
            },
        ],
    );
    let doc = ForgeDoc::from_bytes(doc.canonical_json().unwrap().as_bytes(), &limits).unwrap();
    replay(&doc, &limits, &mut target).unwrap();

    editor
        .apply(Command::set(block, 0, "scale", ScenarioValue::Float(2.0)))
        .unwrap();
    let bytes = editor
        .scenario()
        .unwrap()
        .new_entry(block, Some(1), Some(position))
        .unwrap();
    editor.apply(Command::add(block, bytes)).unwrap();
    editor
        .apply(Command::set(
            block,
            2,
            "palette index",
            ScenarioValue::Int(0),
        ))
        .unwrap();
    editor.apply(Command::remove(block, 1)).unwrap();
    editor
        .apply(Command::set(
            BlockKind::NetgameEquipment,
            0,
            "item",
            ScenarioValue::Tag(TagRef {
                group: FourCC(*b"weap"),
                id: id(2),
            }),
        ))
        .unwrap();

    assert_eq!(
        target.session.scenario().unwrap().changes(),
        editor.scenario().unwrap().changes()
    );
    assert_eq!(
        output(&target.source, &target.session),
        output(&target.source, &editor)
    );
}

#[test]
fn a_portable_tag_key_must_resolve_before_a_placement_is_written() {
    let (session, source) = open();
    let mut target = ScenarioApply { session, source };
    let doc = draft(
        &target.source,
        vec![Placement::Add {
            block: BlockKind::Weapons,
            like: Some(1),
            fields: vec![FieldWrite {
                field: "palette index".into(),
                value: Value::Tag(Some(TagKey {
                    group: "weap".into(),
                    path: "missing".into(),
                })),
            }],
        }],
    );
    assert!(replay(&doc, &Limits::default(), &mut target).is_err());
    assert!(!target.session.scenario().unwrap().is_modified());
}
