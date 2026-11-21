use std::io::{Read, Seek};

use forge_doc::{
    Apply, BlockKind, F32, FieldWrite, Placement, Refusal, TagKey, TagWrite, Value, WorldBounds,
};
use map::{FourCC, Map, Tag, TagQuery};
use scenario_edit::{
    Command, Session, SetField, TagRef, Value as ScenarioValue, edit,
    geometry::{Bounds, scenario_bsps},
    layout::{self, Role},
};
use tags::structure::StructureRender;

use crate::{Error, tag_fields::TagFields};

pub(super) fn invalid(what: &str, error: impl std::fmt::Display) -> Refusal {
    Refusal::Invalid {
        what: what.into(),
        reason: error.to_string(),
    }
}

pub(super) fn lookup<'a, R: Read + Seek>(
    map: &'a Map<R>,
    key: &TagKey,
) -> forge_doc::Result<&'a Tag> {
    let group = FourCC(
        key.group
            .as_bytes()
            .try_into()
            .map_err(|e| invalid("tag group", e))?,
    );
    map.tag(TagQuery::NameInGroup(&key.path, group))
        .map_err(|e| invalid(&format!("{}.{}", key.path, key.group), e))
}

pub(super) fn world_bounds<R: Read + Seek>(map: &mut Map<R>) -> Result<Option<WorldBounds>, Error> {
    let mut world = Bounds::EMPTY;
    for bsp in scenario_bsps(map)? {
        let structure = StructureRender::read(map, bsp).map_err(scenario_edit::Error::from)?;
        for cluster in &structure.clusters {
            let [x0, x1, y0, y1, z0, z1] = cluster.bounds;
            world.add([x0, y0, z0]);
            world.add([x1, y1, z1]);
        }
    }
    Ok((!world.is_empty()).then_some(WorldBounds {
        min: world.min,
        max: world.max,
    }))
}

pub(super) struct Materialiser<'a, R> {
    session: &'a mut Session,
    map: &'a mut Map<R>,
    tags: TagFields,
}

impl<'a, R: Read + Seek> Materialiser<'a, R> {
    pub fn new(session: &'a mut Session, map: &'a mut Map<R>) -> Self {
        Self {
            session,
            map,
            tags: TagFields::default(),
        }
    }

    pub fn finish(self) -> Result<Vec<String>, Error> {
        self.session
            .set_tag_patches(self.tags.patches.into_iter().collect())?;
        Ok(self.tags.strings)
    }

    fn scenario(&self) -> forge_doc::Result<&scenario_edit::Scenario> {
        self.session
            .scenario()
            .ok_or_else(|| invalid("placement", "scenario is not open"))
    }

    fn reference(&self, key: Option<&TagKey>) -> forge_doc::Result<TagRef> {
        match key {
            Some(key) => {
                let tag = lookup(self.map, key)?;
                Ok(TagRef {
                    group: tag.group,
                    id: tag.id,
                })
            }
            None => Ok(TagRef::NONE),
        }
    }

    fn field(
        &self,
        block: BlockKind,
        index: usize,
        write: &FieldWrite,
    ) -> forge_doc::Result<SetField> {
        let field = layout::block(block)
            .field(&write.field)
            .ok_or_else(|| invalid(block.name(), "unknown placement field"))?;
        let value = match &write.value {
            Value::Int(n) => ScenarioValue::Int(*n),
            Value::Float(n) => ScenarioValue::Float(n.get()),
            Value::Vec3(v) => ScenarioValue::Vec3(v.map(F32::get)),
            Value::Tag(key) if field.role == Role::PaletteIndex => {
                let tag = self.reference(key.as_ref())?;
                let index = if tag.is_none() {
                    -1
                } else {
                    self.scenario()?
                        .block(block)
                        .palette
                        .iter()
                        .position(|entry| *entry == tag)
                        .ok_or_else(|| {
                            invalid(field.name, "tag is absent from the block's palette")
                        })? as i64
                };
                ScenarioValue::Int(index)
            }
            Value::Tag(key) => ScenarioValue::Tag(self.reference(key.as_ref())?),
            _ => return Err(invalid(field.name, "wrong placement field type")),
        };
        Ok(SetField {
            block,
            index,
            field: field.name,
            value,
        })
    }
}

impl<R: Read + Seek> Apply for Materialiser<'_, R> {
    fn placement(&mut self, placement: &Placement) -> forge_doc::Result<()> {
        let block = placement.block();
        let command = match placement {
            Placement::Set { index, write, .. } => {
                let set = self.field(block, *index as usize, write)?;
                Command::set(block, set.index, set.field, set.value)
            }
            Placement::Add { like, fields, .. } => {
                let scenario = self.scenario()?;
                let index = scenario.block(block).len();
                let entry = scenario
                    .new_entry(block, like.map(|n| n as usize), None)
                    .map_err(|e| invalid(block.name(), e))?;
                let mut command = Command::add(block, entry);
                for write in fields {
                    command
                        .ops
                        .push(edit::Op::Set(self.field(block, index, write)?));
                }
                command
            }
            Placement::Remove { index, .. } => Command::remove(block, *index as usize),
        };
        self.session
            .apply(command)
            .map_err(|e| invalid(block.name(), e))
    }

    fn tag_field(&mut self, write: &TagWrite) -> forge_doc::Result<()> {
        self.tags.write(self.map, write)
    }
}
