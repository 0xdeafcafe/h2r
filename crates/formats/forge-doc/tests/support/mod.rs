//! An `Apply` adapter over the editor's session on its invented cache,
//! shared by the parity tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Cursor;

use forge_doc::{
    Apply, BlockKind, BuilderVersions, Edit, F32, FieldWrite, ForgeDoc, MapRef, Op, OpId,
    Placement, Printed, Refusal, Sha256, TagKey, TagWrite, Value, check_print,
};
use map::{FourCC, TagQuery};
use scenario_edit::{
    Command, MapSource, Session, SetField, TagRef, Value as ScenarioValue, edit,
    fixture::fixture,
    layout::{self, Role},
    save,
};

pub fn open() -> (Session, MapSource) {
    let mut source = MapSource::file("forge-fixture/fixture.map");
    source.bytes = Some(fixture().into());
    let mut session = Session::new();
    session.build_scenes = false;
    session.open_named(source.clone(), "fixture").unwrap();
    (session, source)
}

pub fn editor_error(error: impl std::fmt::Display) -> Refusal {
    Refusal::Invalid {
        what: "scenario".into(),
        reason: error.to_string(),
    }
}

/// The base count of every block of the session's scenario.
pub fn base_counts(session: &Session) -> BTreeMap<BlockKind, u32> {
    layout::BLOCKS
        .iter()
        .map(|def| {
            (
                def.kind,
                session.scenario().unwrap().block(def.kind).len() as u32,
            )
        })
        .collect()
}

pub struct ScenarioApply {
    pub session: Session,
    pub source: MapSource,
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

    /// The tag key a reference names, `None` for none.
    fn key(&self, reference: TagRef) -> forge_doc::Result<Option<TagKey>> {
        if reference.is_none() {
            return Ok(None);
        }
        let map = self.source.open().map_err(editor_error)?;
        let tag = map.tag(reference.id).map_err(editor_error)?;
        Ok(Some(TagKey {
            group: String::from_utf8_lossy(&tag.group.0).into_owned(),
            path: tag
                .name
                .clone()
                .ok_or_else(|| editor_error("unnamed tag"))?,
        }))
    }

    /// The entry's typed fields as a print reads them.
    pub fn values(&self, block: BlockKind, index: usize) -> forge_doc::Result<Vec<Value>> {
        let scenario = self.session.scenario().unwrap();
        let mut values = Vec::new();
        for field in layout::block(block).fields {
            let value = scenario
                .get(block, index, field.name)
                .map_err(editor_error)?;
            values.push(match value {
                ScenarioValue::Int(-1) if field.role == Role::PaletteIndex => Value::Tag(None),
                ScenarioValue::Int(n) if field.role == Role::PaletteIndex => {
                    let entry = usize::try_from(n)
                        .ok()
                        .and_then(|n| scenario.block(block).palette.get(n))
                        .ok_or_else(|| editor_error("not an entry of the palette"))?;
                    Value::Tag(self.key(*entry)?)
                }
                ScenarioValue::Int(n) => Value::Int(n),
                ScenarioValue::Float(x) => Value::Float(F32::new(x)?),
                ScenarioValue::Vec3(v) => {
                    Value::Vec3([F32::new(v[0])?, F32::new(v[1])?, F32::new(v[2])?])
                }
                ScenarioValue::Tag(reference) => Value::Tag(self.key(reference)?),
            });
        }
        Ok(values)
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

    fn prints(&mut self, prints: &[Printed]) -> forge_doc::Result<()> {
        for printed in prints {
            let fields = self.values(printed.block, printed.index as usize)?;
            check_print(printed.block, printed.index, printed.print, &fields)?;
        }
        Ok(())
    }
}

pub fn output(source: &MapSource, session: &Session) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    save::write_patched(
        &mut source.open().unwrap(),
        session.scenario().unwrap(),
        &mut out,
    )
    .unwrap();
    out.into_inner()
}

/// A document over the fixture with `edits` as ops 1, 2, ... at the
/// format they need.
pub fn draft(source: &MapSource, edits: Vec<Edit>) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
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
                edit,
            })
            .collect(),
        expect: None,
    };
    doc.format = doc.required_format();
    doc
}
