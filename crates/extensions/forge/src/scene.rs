//! Resolve the authored suffix of each placement block in scenario order.
//! The builder appends on Add and swap-removes within one block on Remove.
use std::collections::BTreeMap;

use crate::{BlockKind, Frame, OpId, PaletteItem, PlacedObject, World};
use forge_doc::{Edit, F32, FieldWrite, ForgeDoc, Limits, Placement, Value};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Object {
    id: OpId,
    item: PaletteItem,
    frame: Frame,
}

impl From<&PlacedObject> for Object {
    fn from(object: &PlacedObject) -> Self {
        Self {
            id: object.id,
            item: object.item.clone(),
            frame: object.frame,
        }
    }
}

pub(crate) fn project(
    doc: &ForgeDoc,
    limits: &Limits,
    palette: &[PaletteItem],
    budget: usize,
) -> Result<Vec<Object>, String> {
    let mut blocks: BTreeMap<BlockKind, Vec<Object>> = BTreeMap::new();
    for op in &doc.ops {
        let placement = match &op.edit {
            Edit::Placement(placement) => placement,
            // Links are retail data on scenario entries; they place nothing.
            Edit::Forge(_) => continue,
            _ => return Err("The Objects tool can load placement edits only.".into()),
        };
        let block = placement.block();
        let base = *limits
            .base_counts
            .get(&block)
            .ok_or("Missing base placement count.")?;
        let rows = blocks.entry(block).or_default();
        let offset = |index: u32| {
            index
                .checked_sub(base)
                .map(|n| n as usize)
                .ok_or_else(|| "The Objects tool edits Forge placements only.".to_string())
        };
        match placement {
            Placement::Add { like, fields, .. } => {
                if like.is_some() {
                    return Err("Copying an existing placement needs a rebuild.".into());
                }
                let mut item = None;
                let mut frame = Frame::default();
                for field in fields {
                    if let ("palette index", Value::Tag(Some(tag))) =
                        (field.field.as_str(), &field.value)
                    {
                        item = palette
                            .iter()
                            .find(|item| item.block == block && &item.tag == tag);
                        if item.is_none() {
                            return Err(format!(
                                "{} is absent from the {} palette.",
                                tag.path,
                                block.name()
                            ));
                        }
                    } else {
                        update(&mut frame, field)?;
                    }
                }
                if let Some(bounds) = limits.world
                    && !frame
                        .position
                        .iter()
                        .zip(bounds.min.into_iter().zip(bounds.max))
                        .all(|(x, (lo, hi))| (lo..=hi).contains(x))
                {
                    return Err("Object position lies outside the BSP bounds.".into());
                }
                rows.push(Object {
                    id: op.id,
                    item: item.ok_or("A Forge object needs a palette tag.")?.clone(),
                    frame,
                });
            }
            Placement::Set { index, write, .. } => {
                let object = rows
                    .get_mut(offset(*index)?)
                    .ok_or("No such Forge placement.")?;
                update(&mut object.frame, write)?;
            }
            Placement::Remove { index, .. } => {
                let offset = offset(*index)?;
                if offset >= rows.len() {
                    return Err("No such Forge placement.".into());
                }
                rows.swap_remove(offset);
            }
        }
    }
    let count: usize = blocks.values().map(Vec::len).sum();
    if count > budget {
        return Err(format!("Object budget exceeded ({count} / {budget})."));
    }
    Ok(blocks.into_values().flatten().collect())
}

fn update(frame: &mut Frame, write: &FieldWrite) -> Result<(), String> {
    let target = match write.field.as_str() {
        "position" => &mut frame.position,
        "rotation" => &mut frame.rotation,
        name => return Err(format!("The Objects tool cannot apply {name}.")),
    };
    let Value::Vec3(v) = &write.value else {
        return Err("Object transform requires three coordinates.".into());
    };
    *target = v.map(F32::get);
    Ok(())
}

/// Keep handles current after each successful call. This lets a failed
/// multi-object load compensate from the exact point where it stopped.
pub(crate) fn reconcile(
    world: &mut impl World,
    live: &mut Vec<PlacedObject>,
    desired: &[Object],
    respawn: bool,
) -> Result<(), String> {
    let wanted: BTreeMap<_, _> = desired.iter().map(|row| (row.id, row)).collect();
    let mut i = 0;
    while i < live.len() {
        let old = &live[i];
        if wanted.get(&old.id).is_some_and(|row| row.item == old.item) {
            i += 1;
        } else {
            world.delete(old.handle)?;
            live.remove(i);
        }
    }
    let present: BTreeMap<_, _> = live
        .iter()
        .enumerate()
        .map(|(i, row)| (row.id, i))
        .collect();
    for row in desired {
        if let Some(&i) = present.get(&row.id) {
            let object = &mut live[i];
            if object.frame != row.frame {
                world.move_object(object.handle, row.frame)?;
                object.frame = row.frame;
            }
        } else {
            let handle = if respawn {
                world.respawn(&row.item, row.frame)?
            } else {
                world.place(&row.item, row.frame)?
            };
            live.push(PlacedObject {
                id: row.id,
                handle,
                item: row.item.clone(),
                frame: row.frame,
            });
        }
    }
    // Objects() follows the scenario row order even after a middle deletion.
    let order: BTreeMap<_, _> = desired
        .iter()
        .enumerate()
        .map(|(i, row)| (row.id, i))
        .collect();
    live.sort_by_key(|row| order[&row.id]);
    Ok(())
}
