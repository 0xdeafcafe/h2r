//! Resolve the added entries of each placement block in scenario order.
//! The builder appends on Add and swap-removes within one block on Remove,
//! whether the entry removed is an added one or the map's own.
use std::collections::BTreeMap;

use crate::{BlockKind, Frame, OpId, PaletteItem, PlacedObject, World};
use forge_doc::{Edit, Endpoint, F32, FieldWrite, Forge, ForgeDoc, Limits, Placement, Value};
use forge_shapes::PieceDef;
use map::scenario_layout as layout;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Object {
    pub id: OpId,
    pub item: PaletteItem,
    pub frame: Frame,
    /// The definition of a block-out piece, whose `item` is then its
    /// definition's own `bloc` (`crate::piece::item`). A piece makes an
    /// object only where the world holds that definition.
    pub piece: Option<PieceDef>,
}

impl From<&PlacedObject> for Object {
    fn from(object: &PlacedObject) -> Self {
        Self {
            id: object.id,
            item: object.item.clone(),
            frame: object.frame,
            piece: None,
        }
    }
}

/// A netgame flag the document adds. It has no live object in the session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Marker {
    pub id: OpId,
    /// Its index in the netgame flags after every op.
    pub index: u32,
    /// Position and facing (`rotation[0]`).
    pub frame: Frame,
    pub kind: u16,
    pub team: u16,
    pub identifier: i16,
}

/// A starting location or netgame equipment entry the document adds: a
/// row of the kit. It has no live object in the session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub id: OpId,
    pub block: BlockKind,
    /// Its index in its block after every op.
    pub index: u32,
    /// The fields an add or a set wrote, by layout name, in the layout's
    /// order. Any other field is as a zeroed row has it.
    pub set: Vec<(&'static str, Value)>,
}

/// The added objects a document leaves, and where each is.
pub(crate) struct Scene {
    /// By block, then by index in the block.
    pub objects: Vec<Object>,
    /// The added netgame flags.
    pub markers: Vec<Marker>,
    /// The added starting locations and netgame equipment entries.
    pub entries: Vec<Entry>,
    /// Each added object's index in its block after every op: the running
    /// index a placement op by index names it at.
    pub index: BTreeMap<OpId, u32>,
}

/// One row of a block as the document's ops leave it: the base map's own
/// entry, or one an add made.
enum Row {
    Own,
    Added(Object),
    Marker(Marker),
    Entry(Entry),
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
            // A piece's definition names its placement's object.
            Edit::Forge(Forge::Piece { target, def }) => {
                if let Endpoint::Added(id) = target
                    && let Some(rows) = blocks.get_mut(&crate::BlockKind::Crates)
                {
                    for row in rows {
                        if let Row::Added(object) = row
                            && object.id == *id
                        {
                            object.item = crate::piece::item(def);
                            object.piece = Some(def.clone());
                        }
                    }
                }
                continue;
            }
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
            Placement::Add { like, fields, .. }
                if block == forge_doc::GROUND_BLOCK
                    && !fields.iter().any(|f| f.field == "palette index") =>
            {
                if like.is_some() {
                    return Err("Copying an existing placement needs a rebuild.".into());
                }
                let mut marker = Marker {
                    id: op.id,
                    index: 0,
                    frame: Frame::default(),
                    kind: 0,
                    team: 0,
                    identifier: 0,
                };
                for field in fields {
                    update_marker(&mut marker, field)?;
                }
                in_bounds(limits, marker.frame)?;
                rows.push(Row::Marker(marker));
            }
            Placement::Add { like, fields, .. } if crate::kit::is_block(block) => {
                // Starting locations and netgame equipment (the flags are
                // the marker arm's) are rows of the kit.
                if like.is_some() {
                    return Err("Copying an existing placement needs a rebuild.".into());
                }
                let mut entry = Entry {
                    id: op.id,
                    block,
                    index: 0,
                    set: Vec::new(),
                };
                for field in fields {
                    update_entry(&mut entry, field)?;
                }
                rows.push(Row::Entry(entry));
            }
            Placement::Add { like, fields, .. } => {
                if like.is_some() {
                    return Err("Copying an existing placement needs a rebuild.".into());
                }
                let mut item = None;
                let mut frame = Frame::default();
                for field in fields {
                    if field.field == "palette index" {
                        if is_piece_tag(block, &field.value) {
                            // The definition comes with its own op.
                            item = Some(piece_placeholder());
                        } else {
                            item = Some(palette_item(palette, block, &field.value)?.clone());
                        }
                    } else {
                        update(&mut frame, field)?;
                    }
                }
                in_bounds(limits, frame)?;
                rows.push(Row::Added(Object {
                    id: op.id,
                    item: item.ok_or("A Forge object needs a palette tag.")?,
                    frame,
                    piece: None,
                }));
            }
            Placement::Set { index, write, .. } => match rows.get_mut(*index as usize) {
                Some(Row::Added(object)) => {
                    if write.field == "palette index" {
                        if object.piece.is_some() || is_piece_tag(block, &write.value) {
                            return Err("A piece's tag is its definition's.".into());
                        }
                        object.item = palette_item(palette, block, &write.value)?.clone();
                    } else {
                        update(&mut object.frame, write)?;
                        in_bounds(limits, object.frame)?;
                    }
                }
                Some(Row::Marker(marker)) => {
                    update_marker(marker, write)?;
                    in_bounds(limits, marker.frame)?;
                }
                Some(Row::Entry(entry)) => update_entry(entry, write)?,
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
        markers: Vec::new(),
        entries: Vec::new(),
        index: BTreeMap::new(),
    };
    for rows in blocks.into_values() {
        for (i, row) in rows.into_iter().enumerate() {
            match row {
                Row::Added(object) => {
                    if object.item == piece_placeholder() {
                        return Err("A piece has no definition.".into());
                    }
                    scene.index.insert(object.id, i as u32);
                    scene.objects.push(object);
                }
                Row::Marker(mut marker) => {
                    marker.index = i as u32;
                    scene.index.insert(marker.id, i as u32);
                    scene.markers.push(marker);
                }
                Row::Entry(mut entry) => {
                    entry.index = i as u32;
                    scene.index.insert(entry.id, i as u32);
                    scene.entries.push(entry);
                }
                Row::Own => {}
            }
        }
    }
    // A link's channel is the identifier of the flags it joins, as the
    // materialiser writes it. A link that does not resolve is the
    // session's to report (`Session::channel_changes`).
    if let Ok(links) = doc.links(&limits.base_counts) {
        for active in &links.active {
            if let forge_doc::Link::Teleporter {
                sender,
                receiver,
                channel,
            } = active.link
            {
                for end in [sender, receiver] {
                    if let forge_doc::Endpoint::Added(id) = end
                        && let Some(marker) = scene.markers.iter_mut().find(|m| m.id == id)
                        && let Ok(channel) = i16::try_from(channel)
                    {
                        marker.identifier = channel;
                    }
                }
            }
        }
    }
    let count = scene.objects.len();
    if count > budget {
        return Err(format!("Object budget exceeded ({count} / {budget})."));
    }
    Ok(scene)
}

/// What a piece's placement holds until its definition's op names the
/// object (`forge_doc::PIECE_TAG`).
fn piece_placeholder() -> PaletteItem {
    PaletteItem {
        block: crate::BlockKind::Crates,
        tag: forge_doc::piece_tag(),
    }
}

/// Whether `value` is the piece tag written to a palette index of `block`.
fn is_piece_tag(block: BlockKind, value: &Value) -> bool {
    block == BlockKind::Crates && *value == forge_doc::piece_value()
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

/// One field of an added netgame flag.
fn update_marker(marker: &mut Marker, write: &FieldWrite) -> Result<(), String> {
    match (write.field.as_str(), &write.value) {
        ("position", Value::Vec3(v)) => marker.frame.position = v.map(F32::get),
        ("facing", Value::Float(f)) => marker.frame.rotation[0] = f.get(),
        ("type", Value::Int(kind)) => {
            marker.kind = u16::try_from(*kind).map_err(|_| "A flag's type is out of range.")?;
        }
        ("team", Value::Int(team)) => {
            marker.team = u16::try_from(*team).map_err(|_| "A flag's team is out of range.")?;
        }
        ("identifier", Value::Int(id)) => {
            marker.identifier = i16::try_from(*id).map_err(|_| "A flag's identifier is out of range.")?;
        }
        (name, _) => return Err(format!("A marker has no {name} to set here.")),
    }
    Ok(())
}

/// One field of an added kit entry: the layout's editable field of that
/// name, kept in the layout's order.
fn update_entry(entry: &mut Entry, write: &FieldWrite) -> Result<(), String> {
    let def = layout::block(entry.block);
    let at = def
        .fields
        .iter()
        .position(|f| f.name == write.field && f.editable)
        .ok_or_else(|| format!("A {} has no {} to set here.", crate::noun(entry.block).0, write.field))?;
    let name = def.fields[at].name;
    match entry.set.iter_mut().find(|(n, _)| *n == name) {
        Some((_, value)) => *value = write.value.clone(),
        None => {
            let next = entry
                .set
                .iter()
                .position(|(n, _)| {
                    def.fields.iter().position(|f| f.name == *n).is_some_and(|i| i > at)
                })
                .unwrap_or(entry.set.len());
            entry.set.insert(next, (name, write.value.clone()));
        }
    }
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
        // A piece that moves is made again where it lands: its body is fixed
        // where its object was made.
        if wanted.get(&old.id).is_some_and(|row| {
            row.item == old.item && (row.piece.is_none() || row.frame == old.frame)
        }) {
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
            // A piece the loaded map does not hold has no object: it is
            // pending until the map is rebuilt.
            if let Some(def) = &row.piece
                && !world.piece_loaded(&def.name())
            {
                continue;
            }
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
