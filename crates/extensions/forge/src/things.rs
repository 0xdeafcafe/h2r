//! The things of a session: every entry of the loaded map's placement
//! blocks, own and added (docs/game/forge-map-variants.md, "Things" and
//! "Applying an edit in a session").
//!
//! The host reads the loaded scenario's rows once, when the session opens
//! ([`BaseRows`], `Session::with_base`): each row's raw bytes, its typed
//! fields and its print. The session keeps them in its own memory, never in
//! the document, the log, a save or the network. An entry op on an own entry
//! is checked against the kept print, and an undo or a `Reset` puts the kept
//! row back exactly.
//!
//! The world's state of the own entries is kept beside the document's: an
//! entry absent from an [`OwnMap`] is as the base map has it. [`reconcile`]
//! makes the base calls that take the world from one state to another, and
//! records each call as it succeeds, so a refused call can be compensated
//! from exactly where it stopped.
use std::collections::{BTreeMap, BTreeSet};

use forge_doc::{BlockKind, Edit, Endpoint, EntryOp, F32, FieldWrite, ForgeDoc, Print, Value};
use map::scenario_layout::{self as layout, Role};

use crate::{Frame, World};

/// What a thing is to a player (docs/game/forge-map-variants.md, "Things").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThingKind {
    /// A scenery, crate, machine, weapon, vehicle or equipment placement.
    Object,
    /// A netgame equipment entry.
    ItemSpawner,
    /// A player starting location.
    StartingPoint,
    /// A netgame flag of a goal type.
    Goal,
    /// A netgame flag of the teleporter sender or receiver type.
    Teleporter,
}

impl ThingKind {
    /// The kind of an entry of `block` with these typed `fields`, or none
    /// for a block that holds no things: sound scenery has no body and
    /// starting equipment no decoded field worth a row. A netgame flag is a
    /// teleporter when its type is 7 (sender) or 8 (receiver), else a goal.
    pub fn of(block: BlockKind, fields: &[Value]) -> Option<Self> {
        Some(match block {
            BlockKind::Scenery
            | BlockKind::Vehicles
            | BlockKind::Equipment
            | BlockKind::Weapons
            | BlockKind::Machines
            | BlockKind::Crates => Self::Object,
            BlockKind::NetgameEquipment => Self::ItemSpawner,
            BlockKind::StartingLocations => Self::StartingPoint,
            BlockKind::NetgameFlags => {
                let at = layout::block(block)
                    .fields
                    .iter()
                    .position(|f| f.name == "type")?;
                match fields.get(at) {
                    Some(Value::Int(kind)) => Self::flag(*kind as u16),
                    _ => Self::Goal,
                }
            }
            BlockKind::SoundScenery | BlockKind::StartingEquipment => return None,
        })
    }

    pub(crate) fn flag(kind: u16) -> Self {
        match kind {
            forge_doc::TELEPORTER_SENDER | forge_doc::TELEPORTER_RECEIVER => Self::Teleporter,
            _ => Self::Goal,
        }
    }

    /// The kind as a player reads it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::ItemSpawner => "item spawner",
            Self::StartingPoint => "starting point",
            Self::Goal => "goal",
            Self::Teleporter => "teleporter",
        }
    }
}

/// One entry of the loaded map as the session opened it.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseRow {
    pub block: BlockKind,
    /// The entry's index in the block, as the disc has it.
    pub index: u32,
    /// The entry's bytes, the layout's stride of them.
    pub raw: Vec<u8>,
    /// One value for each of the layout's fields, in its order.
    pub fields: Vec<Value>,
    /// `forge_doc::print(block, &fields)`.
    pub print: Print,
}

/// The loaded map's rows, as the host read them when the session opened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BaseRows {
    pub rows: Vec<BaseRow>,
    /// Each block that did not read, as `block: reason`.
    pub left_out: Vec<String>,
}

/// The end of the refusal of an edit that names an entry of a block left
/// out of the things: "The map's <block> entries " comes before it.
pub const LEFT_OUT_SENTENCE: &str = "did not read, so Forge cannot edit them";

/// The rows the session keeps, by block and base index. A block is kept
/// whole or not at all.
#[derive(Clone, Debug, Default)]
pub(crate) struct Kept {
    rows: BTreeMap<BlockKind, Vec<BaseRow>>,
    pub left_out: Vec<String>,
}

impl Kept {
    /// Keep every block of `base` whose rows are the map's: one for each of
    /// the base map's entries, in index order, of the layout's width, with
    /// the layout's fields and their print. Any other block is left out by
    /// name.
    pub fn new(base: BaseRows, counts: &BTreeMap<BlockKind, u32>) -> Self {
        let mut blocks: BTreeMap<BlockKind, Vec<BaseRow>> = BTreeMap::new();
        for row in base.rows {
            blocks.entry(row.block).or_default().push(row);
        }
        let mut kept = Self {
            rows: BTreeMap::new(),
            left_out: base.left_out,
        };
        for (block, mut rows) in blocks {
            rows.sort_by_key(|row| row.index);
            match check_block(block, &rows, counts.get(&block).copied()) {
                Ok(()) => {
                    kept.rows.insert(block, rows);
                }
                Err(reason) => kept.left_out.push(format!("{}: {reason}", block.name())),
            }
        }
        kept
    }

    /// The kept row of the base entry `(block, index)`.
    pub fn row(&self, block: BlockKind, index: u32) -> Result<&BaseRow, String> {
        let rows = self.rows.get(&block).ok_or_else(|| {
            format!(
                "The map's {} entries {LEFT_OUT_SENTENCE}.",
                block.name().replace('_', " ")
            )
        })?;
        rows.get(index as usize).ok_or_else(|| {
            format!(
                "The map has no {} entry {index}.",
                block.name().replace('_', " ")
            )
        })
    }

    pub(crate) fn flag_types(&self) -> Option<std::sync::Arc<[u16]>> {
        let block = BlockKind::NetgameFlags;
        let at = field_at(block, "type").ok()?;
        self.rows
            .get(&block)?
            .iter()
            .map(|row| match row.fields.get(at) {
                Some(Value::Int(kind)) => u16::try_from(*kind).ok(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(Into::into)
    }

    /// Every kept row, in the layout's block order and then index order.
    pub fn all(&self) -> impl Iterator<Item = &BaseRow> {
        layout::BLOCKS
            .iter()
            .filter_map(|def| self.rows.get(&def.kind))
            .flatten()
    }
}

fn check_block(block: BlockKind, rows: &[BaseRow], count: Option<u32>) -> Result<(), String> {
    let count = count.ok_or("the map has no count for the block")?;
    if rows.len() != count as usize {
        return Err(format!(
            "the loaded scenario has {} rows, the map {count}",
            rows.len()
        ));
    }
    let def = layout::block(block);
    for (i, row) in rows.iter().enumerate() {
        if row.index as usize != i {
            return Err(format!("row {i} is missing"));
        }
        if row.raw.len() != def.stride as usize {
            return Err(format!("row {i} is not {} bytes", def.stride));
        }
        if forge_doc::print(block, &row.fields).ok() != Some(row.print) {
            return Err(format!("row {i}'s print is not its fields'"));
        }
    }
    Ok(())
}

/// One thing of the session: an own entry of the loaded map or an entry the
/// document added, with its fields as the document leaves them.
#[derive(Clone, Debug, PartialEq)]
pub struct Thing {
    /// An own entry is `Endpoint::Base` with its print; an added one is
    /// `Endpoint::Added` with its add op's ID.
    pub id: Endpoint,
    pub block: BlockKind,
    pub kind: ThingKind,
    /// The base map has it. Only an own entry can be restored.
    pub own: bool,
    pub frame: Frame,
    /// The layout's fields by name, in its order. An added entry has its
    /// palette entry, position and rotation.
    pub fields: Vec<(&'static str, Value)>,
    /// An entry op of the document changes it. An added entry is a change.
    pub changed: bool,
}

/// An own entry's state where it differs from the base map's. A removed
/// entry keeps the base map's fields, which a restore writes back.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Own {
    pub fields: Vec<Value>,
    pub removed: bool,
}

/// Own entries by `(block, base index)`. An entry absent is as the base map
/// has it.
pub(crate) type OwnMap = BTreeMap<(BlockKind, u32), Own>;

/// The own entries' state `doc` gives: each base entry an active entry op
/// names, with its writes and removal applied in op order. Every print the
/// document carries is checked against the kept row first
/// (`Refusal::Print`), and an entry whose block did not read is refused.
pub(crate) fn desired(
    doc: &ForgeDoc,
    counts: &BTreeMap<BlockKind, u32>,
    kept: &Kept,
) -> Result<OwnMap, String> {
    let resolved = forge_doc::resolve(&doc.ops, counts).map_err(|e| e.to_string())?;
    for printed in &resolved.prints {
        let row = kept.row(printed.block, printed.index)?;
        forge_doc::check_print(printed.block, printed.index, printed.print, &row.fields)
            .map_err(|e| e.to_string())?;
    }
    let mut out = OwnMap::new();
    for op in &doc.ops {
        let Edit::Entry(edit) = &op.edit else {
            continue;
        };
        let Endpoint::Base { block, index, .. } = edit.target() else {
            continue;
        };
        // A reset, and every op a later reset covers, has no effect.
        if !resolved.lowered.contains_key(&op.id) {
            continue;
        }
        let row = kept.row(block, index)?;
        let own = out.entry((block, index)).or_insert_with(|| Own {
            fields: row.fields.clone(),
            removed: false,
        });
        match edit {
            EntryOp::Set { write, .. } => {
                let at = field_at(block, &write.field)?;
                own.fields[at] = write.value.clone();
            }
            EntryOp::Remove { .. } => own.removed = true,
            EntryOp::Reset { .. } | EntryOp::Property { .. } | EntryOp::Area { .. } => {}
        }
    }
    for ((block, index), own) in &mut out {
        if own.removed {
            own.fields = kept.row(*block, *index)?.fields.clone();
        }
    }
    Ok(out)
}

fn field_at(block: BlockKind, name: &str) -> Result<usize, String> {
    layout::block(block)
        .fields
        .iter()
        .position(|f| f.name == name)
        .ok_or_else(|| format!("{} has no field {name:?}.", block.name()))
}

/// The layout fields a frame sets on an entry of `block`: its position, and
/// its rotation (yaw, pitch, roll) or its facing (the yaw).
fn frame_fields(block: BlockKind) -> [Option<usize>; 2] {
    let def = layout::block(block);
    let at = |role| def.fields.iter().position(|f| f.role == role);
    [
        at(Role::Position),
        at(Role::Rotation).or_else(|| at(Role::Facing)),
    ]
}

/// The writes that put an entry of `block` at `frame`, in the layout's
/// order: the position, then the rotation or the facing.
pub(crate) fn frame_writes(block: BlockKind, frame: Frame) -> Result<Vec<FieldWrite>, String> {
    let def = layout::block(block);
    let float = |v: f32| F32::new(v).map_err(|e| e.to_string());
    let vec3 = |v: [f32; 3]| -> Result<Value, String> {
        Ok(Value::Vec3([float(v[0])?, float(v[1])?, float(v[2])?]))
    };
    let mut out = Vec::with_capacity(2);
    for at in frame_fields(block).into_iter().flatten() {
        let field = &def.fields[at];
        let value = match field.role {
            Role::Position => vec3(frame.position)?,
            Role::Rotation => vec3(frame.rotation)?,
            _ => Value::Float(float(frame.rotation[0])?),
        };
        out.push(FieldWrite {
            field: field.name.into(),
            value,
        });
    }
    if out.is_empty() {
        return Err(format!(
            "{} entries have no position.",
            block.name().replace('_', " ")
        ));
    }
    Ok(out)
}

/// The frame an entry of `block` with these fields is at. A facing is the
/// yaw, with no pitch or roll.
pub(crate) fn frame_of(block: BlockKind, fields: &[Value]) -> Frame {
    let [position, turn] = frame_fields(block);
    let mut frame = Frame::default();
    if let Some(Value::Vec3(v)) = position.and_then(|at| fields.get(at)) {
        frame.position = v.map(F32::get);
    }
    match turn.and_then(|at| fields.get(at)) {
        Some(Value::Vec3(v)) => frame.rotation = v.map(F32::get),
        Some(Value::Float(yaw)) => frame.rotation = [yaw.get(), 0., 0.],
        _ => {}
    }
    frame
}

/// `fields` with the frame's writes applied.
pub(crate) fn with_frame(
    block: BlockKind,
    fields: &[Value],
    frame: Frame,
) -> Result<Vec<Value>, String> {
    let mut out = fields.to_vec();
    for write in frame_writes(block, frame)? {
        out[field_at(block, &write.field)?] = write.value;
    }
    Ok(out)
}

/// The things of the kept rows, as `own` leaves them, in the layout's block
/// order and then index order. A removed entry is not a thing.
pub(crate) fn own_things(kept: &Kept, own: &OwnMap) -> Vec<Thing> {
    kept.all()
        .filter_map(|row| own_thing(row, own.get(&(row.block, row.index))))
        .collect()
}

/// The thing of one kept row in `state`, or none when it is removed or its
/// block holds no things.
pub(crate) fn own_thing(row: &BaseRow, state: Option<&Own>) -> Option<Thing> {
    if state.is_some_and(|s| s.removed) {
        return None;
    }
    let fields = state.map_or(&row.fields, |s| &s.fields);
    Some(Thing {
        id: Endpoint::Base {
            block: row.block,
            index: row.index,
            print: Some(row.print),
        },
        block: row.block,
        kind: ThingKind::of(row.block, fields)?,
        own: true,
        frame: frame_of(row.block, fields),
        fields: layout::block(row.block)
            .fields
            .iter()
            .map(|f| f.name)
            .zip(fields.iter().cloned())
            .collect(),
        changed: state.is_some(),
    })
}

/// Make the base calls that take the own entries from `live` to `desired`,
/// recording each in `live` as it succeeds. Per entry: a removal is one
/// `base_inert`; a return is one `base_restore` of the kept row; then a
/// changed position or turn is one `base_move`, and each other changed
/// field one `base_field`, in the layout's order.
pub(crate) fn reconcile(
    world: &mut impl World,
    live: &mut OwnMap,
    desired: &OwnMap,
    kept: &Kept,
) -> Result<(), String> {
    let keys: BTreeSet<_> = live.keys().chain(desired.keys()).copied().collect();
    for key in keys {
        let (block, index) = key;
        let row = kept.row(block, index)?;
        let pristine = Own {
            fields: row.fields.clone(),
            removed: false,
        };
        let want = desired.get(&key).unwrap_or(&pristine);
        let mut have = live.get(&key).cloned().unwrap_or_else(|| pristine.clone());
        let record = |live: &mut OwnMap, have: &Own| {
            if *have == pristine {
                live.remove(&key);
            } else {
                live.insert(key, have.clone());
            }
        };
        if want.removed {
            if !have.removed {
                world.base_inert(block, index)?;
                have = Own {
                    fields: row.fields.clone(),
                    removed: true,
                };
                record(live, &have);
            }
            continue;
        }
        if have.removed {
            world.base_restore(block, index, &row.raw)?;
            have = pristine.clone();
            record(live, &have);
        }
        let frame = frame_fields(block);
        if frame
            .iter()
            .flatten()
            .any(|&at| have.fields[at] != want.fields[at])
        {
            world.base_move(block, index, frame_of(block, &want.fields))?;
            for &at in frame.iter().flatten() {
                have.fields[at] = want.fields[at].clone();
            }
            record(live, &have);
        }
        let def = layout::block(block);
        for (at, field) in def.fields.iter().enumerate() {
            if frame.contains(&Some(at)) || have.fields[at] == want.fields[at] {
                continue;
            }
            world.base_field(block, index, field.name, &want.fields[at])?;
            have.fields[at] = want.fields[at].clone();
            record(live, &have);
        }
    }
    Ok(())
}
