//! The materialiser's print check (docs/game/forge-map-variants.md,
//! "Naming an entry"). `forge_doc::replay` hands every base entry the
//! document names with a print to `Apply::prints` before the first edit;
//! the materialiser reads that entry's typed fields from the disc's
//! scenario row and compares prints. A print that differs stops the build
//! as `variant_print` with the block and index, before anything is
//! written. Entry ops themselves arrive lowered to placement ops by
//! running index (`forge_doc::resolve`), which `placement.rs` applies as
//! it applies any other.
use std::io::{Read, Seek};

use forge_doc::{BlockKind, F32, Print, Printed, TagKey, Value};
use map::{Map, TagId};
use scenario_edit::layout::{self, FieldType, Role};

use crate::placement::invalid;

/// The typed fields of one scenario row of `block`, as a print reads them
/// (`forge_doc::print`): one value for each of the layout's fields, in
/// its order. A palette index is the tag key of the palette entry it
/// selects and a tag reference its tag key, through `key`; -1 and a null
/// reference are `Value::Tag(None)`. `palette` is the block's palette as
/// tag IDs.
pub fn entry_fields(
    block: BlockKind,
    row: &[u8],
    palette: &[TagId],
    mut key: impl FnMut(TagId) -> forge_doc::Result<TagKey>,
) -> forge_doc::Result<Vec<Value>> {
    let def = layout::block(block);
    let mut values = Vec::with_capacity(def.fields.len());
    for field in def.fields {
        let at = field.offset as usize;
        let bytes = row
            .get(at..at + field.ty.size() as usize)
            .ok_or_else(|| invalid(block.name(), "the row is shorter than its fields"))?;
        let float = |b: &[u8]| {
            F32::new(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .map_err(|_| invalid(field.name, "the base entry's value is not finite"))
        };
        let mut tag = |id: TagId| {
            if id == TagId::NULL {
                Ok(Value::Tag(None))
            } else {
                key(id).map(|k| Value::Tag(Some(k)))
            }
        };
        values.push(match (field.ty, field.role) {
            (FieldType::I16, Role::PaletteIndex) => {
                match i16::from_le_bytes([bytes[0], bytes[1]]) {
                    -1 => Value::Tag(None),
                    n => {
                        let id = usize::try_from(n)
                            .ok()
                            .and_then(|n| palette.get(n))
                            .ok_or_else(|| invalid(field.name, "not an entry of the palette"))?;
                        tag(*id)?
                    }
                }
            }
            (_, Role::PaletteIndex) => {
                return Err(invalid(field.name, "a palette index is an i16"));
            }
            (FieldType::U8, _) => Value::Int(bytes[0].into()),
            (FieldType::I16, _) => Value::Int(i16::from_le_bytes([bytes[0], bytes[1]]).into()),
            (FieldType::U16, _) => Value::Int(u16::from_le_bytes([bytes[0], bytes[1]]).into()),
            (FieldType::U32, _) => {
                Value::Int(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).into())
            }
            (FieldType::F32, _) => Value::Float(float(bytes)?),
            (FieldType::Vec3, _) => Value::Vec3([
                float(&bytes[0..4])?,
                float(&bytes[4..8])?,
                float(&bytes[8..12])?,
            ]),
            (FieldType::TagRef, _) => tag(TagId(u32::from_le_bytes([
                bytes[4], bytes[5], bytes[6], bytes[7],
            ])))?,
        });
    }
    Ok(values)
}

/// The tag key of a tag of `map`: its group and its name.
pub fn tag_key<R: Read + Seek>(map: &Map<R>, id: TagId) -> forge_doc::Result<TagKey> {
    let tag = map.tag(id).map_err(|e| invalid("tag reference", e))?;
    Ok(TagKey {
        group: tags::defs::fourcc_text(tag.group),
        path: tag
            .name
            .clone()
            .ok_or_else(|| invalid("tag reference", "the tag has no name"))?,
    })
}

/// The typed fields of the base map's entry `index` of `block`: the
/// disc's row, whatever edits the session holds.
pub fn base_fields<R: Read + Seek>(
    map: &Map<R>,
    scenario: &scenario_edit::Scenario,
    block: BlockKind,
    index: u32,
) -> forge_doc::Result<Vec<Value>> {
    let data = scenario.block(block);
    let row = data
        .source_entries()
        .get(index as usize)
        .ok_or_else(|| invalid(block.name(), "names no entry of the base map"))?;
    let palette: Vec<TagId> = data.palette.iter().map(|r| r.id).collect();
    entry_fields(block, row, &palette, |id| tag_key(map, id))
}

/// The print of the base map's entry `index` of `block`.
pub fn base_print<R: Read + Seek>(
    map: &Map<R>,
    scenario: &scenario_edit::Scenario,
    block: BlockKind,
    index: u32,
) -> forge_doc::Result<Print> {
    forge_doc::print(block, &base_fields(map, scenario, block, index)?)
}

/// The check before the first write: the base entry gives the print, or
/// the refusal is `Refusal::Print`.
pub(crate) fn check<R: Read + Seek>(
    map: &Map<R>,
    scenario: &scenario_edit::Scenario,
    printed: &Printed,
) -> forge_doc::Result<()> {
    let fields = base_fields(map, scenario, printed.block, printed.index)?;
    forge_doc::check_print(printed.block, printed.index, printed.print, &fields)
}
