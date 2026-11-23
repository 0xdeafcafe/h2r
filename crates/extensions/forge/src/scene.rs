//! Resolve the added entries of each placement block in scenario order.
//! The builder appends on Add and swap-removes within one block on Remove,
//! whether the entry removed is an added one or the map's own.
use std::collections::BTreeMap;

use crate::{BlockKind, Frame, OpId, PaletteItem, PlacedObject, World};
use forge_doc::{Edit, F32, FieldWrite, ForgeDoc, Limits, Placement, Value};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Object {
    pub id: OpId,
    pub item: PaletteItem,
    pub frame: Frame,
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

/// The added objects a document leaves, and where each is.
pub(crate) struct Scene {
    /// By block, then by index in the block.
    pub objects: Vec<Object>,
    /// Each added object's index in its block after every op: the running
    /// index a placement op by index names it at.
    pub index: BTreeMap<OpId, u32>,
}

/// One row of a block as the document's ops leave it: the base map's own
/// entry, or one an add made.
enum Row {
    Own,
    Added(Object),
}

/// Follow every block an op names through its appends and swap-removes, as
/// the materialiser does. An entry op is applied as the placement op
/// `forge_doc::resolve` lowers it to; on an own entry it moves no object
/// here (`things::desired` follows own entries).
pub(crate) fn project(
    doc: &ForgeDoc,
    limits: &Limits,
    palette: &[PaletteItem],
    budget: usize,
) -> Result<Scene, String> {
    let resolved = forge_doc::resolve(&doc.ops, &limits.base_counts).map_err(|e| e.to_string())?;
    let mut blocks: BTreeMap<BlockKind, Vec<Row>> = BTreeMap::new();
    for op in &doc.ops {
        let (placement, entry) = match &op.edit {
            Edit::Placement(placement) => (placement, false),
            // A reset, and an op a later reset covers, has no effect.
            Edit::Entry(_) => match resolved.lowered.get(&op.id) {
                Some(placement) => (placement, true),
                None => continue,
            },
            // Links and labels are retail data on scenario entries; they
            // place nothing.
            Edit::Forge(_) => continue,
            _ => return Err("The Objects tool can load placement edits only.".into()),
        };
        let block = placement.block();
        let base = *limits
            .base_counts
            .get(&block)
            .ok_or("Missing base placement count.")?;
        let rows = blocks
            .entry(block)
            .or_insert_with(|| (0..base).map(|_| Row::Own).collect());
        match placement {
            Placement::Add { like, fields, .. } => {
                if like.is_some() {
                    return Err("Copying an existing placement needs a rebuild.".into());
                }
                let mut item = None;
                let mut frame = Frame::default();
                for field in fields {
                    if field.field == "palette index" {
                        item = Some(palette_item(palette, block, &field.value)?);
                    } else {
                        update(&mut frame, field)?;
                    }
                }
                in_bounds(limits, frame)?;
                rows.push(Row::Added(Object {
                    id: op.id,
                    item: item.ok_or("A Forge object needs a palette tag.")?.clone(),
                    frame,
                }));
            }
            Placement::Set { index, write, .. } => match rows.get_mut(*index as usize) {
                Some(Row::Added(object)) => {
                    if write.field == "palette index" {
                        object.item = palette_item(palette, block, &write.value)?.clone();
                    } else {
                        update(&mut object.frame, write)?;
                        in_bounds(limits, object.frame)?;
                    }
                }
                // The map's own entry: its fields are the loaded scenario's,
                // not an object of this tool.
                Some(Row::Own) if entry => {}
                Some(Row::Own) => {
                    return Err("The Objects tool edits Forge placements only.".into());
                }
                None => return Err("No such Forge placement.".into()),
            },
            Placement::Remove { index, .. } => match rows.get(*index as usize) {
                Some(Row::Own) if !entry => {
                    return Err("The Objects tool edits Forge placements only.".into());
                }
                Some(_) => {
                    rows.swap_remove(*index as usize);
                }
                None => return Err("No such Forge placement.".into()),
            },
        }
    }
    let mut scene = Scene {
        objects: Vec::new(),
        index: BTreeMap::new(),
    };
    for rows in blocks.into_values() {
        for (i, row) in rows.into_iter().enumerate() {
            if let Row::Added(object) = row {
                scene.index.insert(object.id, i as u32);
                scene.objects.push(object);
            }
        }
    }
    let count = scene.objects.len();
    if count > budget {
        return Err(format!("Object budget exceeded ({count} / {budget})."));
    }
    Ok(scene)
}

/// The palette entry of `block` that a palette index value selects.
fn palette_item<'a>(
    palette: &'a [PaletteItem],
    block: BlockKind,
    value: &Value,
) -> Result<&'a PaletteItem, String> {
    let Value::Tag(Some(tag)) = value else {
        return Err("A Forge object needs a palette tag.".into());
    };
    palette
        .iter()
        .find(|item| item.block == block && &item.tag == tag)
        .ok_or_else(|| format!("{} is absent from the {} palette.", tag.path, block.name()))
}

pub(crate) fn in_bounds(limits: &Limits, frame: Frame) -> Result<(), String> {
    if let Some(bounds) = limits.world
        && !frame
            .position
            .iter()
            .zip(bounds.min.into_iter().zip(bounds.max))
            .all(|(x, (lo, hi))| (lo..=hi).contains(x))
    {
        return Err("Object position lies outside the BSP bounds.".into());
    }
    Ok(())
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
