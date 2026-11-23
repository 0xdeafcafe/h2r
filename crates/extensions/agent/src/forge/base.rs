//! The base calls: edits of the map's own entries in a running game
//! (docs/game/forge-map-variants.md, "Applying an edit in a session").
//!
//! An own entry is a row of one of the loaded scenario's placement blocks.
//! The calls name it by `(block, index)`, the index the base map has it at;
//! a loaded block is never reordered under a running game, so the index
//! holds for the session.
//!
//! - **Move.** The placement's position and orientation are written in the
//!   loaded scenario, so a new round makes the object there, and an object
//!   block's live object is found by its type and placement index (the
//!   object's `+0x1a`, `world.placement.edit`'s `occupant`) and moved with
//!   the `world.object.set` body. An object block whose entry has no live
//!   object is `forge_object_gone`, and nothing is written.
//! - **Field.** One typed, editable field of the row, written in the loaded
//!   scenario; it shows when the object is next made.
//! - **Inert.** The row takes the spare form the Forge kit gives its
//!   reserved rows (`forge-build`'s `kit.rs`, `Block::entry`), which the
//!   engine's readers skip; the live object of an object block is deleted
//!   first. The block keeps its count and its order.
//! - **Restore.** The row the session kept is written back, and an object
//!   block's object is made again, linked to its index (the sequence
//!   `world.placement.edit` creates by).
//!
//! The host stages a [`BaseStep`] on the call queue ([`ForgeWorld`]'s
//! `base_*` methods); at the tick boundary [`plan`] checks every step
//! before the first write, and [`write`] puts the row bytes in.
//!
//! [`ForgeWorld`]: super::local::ForgeWorld
use agent_proto::sim::REFUSED;
use agent_proto::{Kind, ObjectId, RpcError, code};
use engine::GuestMemory;
use forge_doc::{TagKey, Value};
use guest_layout::ObjectType;
use map::scenario_layout::{self as layout, BlockKind, FieldDef, FieldType, Role};

use ::forge::Frame;

use super::local::{OBJECT_GONE, OBJECT_HELD};

/// The widest row of any block (`map::scenario_layout`'s widest stride).
pub const ROW_MAX: usize = 0xa0;

/// The datum of a null tag reference, and a null group.
const NULL: u32 = u32::MAX;

/// One row's bytes, kept by value so a step stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RowBytes {
    len: u8,
    bytes: [u8; ROW_MAX],
}

impl RowBytes {
    /// A row of `block`: exactly the layout's stride of bytes.
    pub fn new(block: BlockKind, row: &[u8]) -> Result<Self, String> {
        let stride = layout::block(block).stride as usize;
        if row.len() != stride || stride > ROW_MAX {
            return Err(format!(
                "a {} row is {stride} bytes, not {}",
                block.name(),
                row.len()
            ));
        }
        let mut bytes = [0; ROW_MAX];
        bytes[..stride].copy_from_slice(row);
        Ok(Self {
            len: stride as u8,
            bytes,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl std::fmt::Debug for RowBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RowBytes({} bytes)", self.len)
    }
}

/// A field value as the row stores it, with any tag already resolved to the
/// salted datum the game holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldValue {
    Int(i64),
    Float(f32),
    Vec3([f32; 3]),
    /// A tag reference field: the group word and the datum.
    Tag {
        group: u32,
        datum: u32,
    },
    /// A palette index field: the datum of the palette entry to select, or
    /// the null datum for none (index -1).
    Palette {
        datum: u32,
    },
}

/// One engine step on an own entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BaseStep {
    /// Position and orientation, in the row and on the live object.
    Move {
        block: BlockKind,
        index: u32,
        frame: Frame,
    },
    /// One editable field of the row.
    Field {
        block: BlockKind,
        index: u32,
        field: &'static str,
        value: FieldValue,
    },
    /// The row's spare form, its live object deleted.
    Inert { block: BlockKind, index: u32 },
    /// The kept row back, its object made again.
    Restore {
        block: BlockKind,
        index: u32,
        row: RowBytes,
    },
}

impl BaseStep {
    pub fn block(&self) -> BlockKind {
        match self {
            Self::Move { block, .. }
            | Self::Field { block, .. }
            | Self::Inert { block, .. }
            | Self::Restore { block, .. } => *block,
        }
    }

    pub fn index(&self) -> u32 {
        match self {
            Self::Move { index, .. }
            | Self::Field { index, .. }
            | Self::Inert { index, .. }
            | Self::Restore { index, .. } => *index,
        }
    }
}

/// The kind and type of the objects a block's rows make, for the six object
/// blocks a live object can be found for.
pub fn object_kind(block: BlockKind) -> Option<(Kind, ObjectType)> {
    Some(match block {
        BlockKind::Scenery => (Kind::Scenery, ObjectType::Scenery),
        BlockKind::Vehicles => (Kind::Vehicle, ObjectType::Vehicle),
        BlockKind::Equipment => (Kind::Equipment, ObjectType::Equipment),
        BlockKind::Weapons => (Kind::Weapon, ObjectType::Weapon),
        BlockKind::Machines => (Kind::Machine, ObjectType::Machine),
        BlockKind::Crates => (Kind::Crate, ObjectType::Crate),
        _ => return None,
    })
}

fn invalid(why: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_REQUEST, why)
}

fn refused(why: impl Into<String>) -> RpcError {
    RpcError::new(REFUSED, why)
}

fn fault(e: engine::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("guest memory fault at {:#x}", e.addr),
    )
}

// ---- the row ----

/// The guest address of row `index` of `block` in the loaded scenario at
/// `scenario`: the index must be inside the block's count, and the block's
/// address must be set.
pub fn row_address(
    m: &dyn GuestMemory,
    scenario: u32,
    block: BlockKind,
    index: u32,
) -> Result<u32, RpcError> {
    let def = layout::block(block);
    let pair = scenario + def.offset;
    let count = m.u32(pair).map_err(fault)?;
    let base = m.u32(pair + 4).map_err(fault)?;
    if index >= count {
        return Err(invalid(format!(
            "the scenario has no {} entry {index}, only {count}",
            block.name()
        )));
    }
    index
        .checked_mul(def.stride)
        .and_then(|at| base.checked_add(at))
        .filter(|at| base != 0 && at.checked_add(def.stride).is_some())
        .ok_or_else(|| invalid(format!("the {} block's address is not set", block.name())))
}

/// The spare form of a row of `block`: the bytes the Forge kit gives its
/// reserved rows, which the engine's readers skip (`forge-build`'s
/// `kit.rs`, `Block::entry`; docs/game/forge-design.md, F3's kit probe).
/// A placement's palette and object-name indices are -1, a starting
/// point's player type and a flag's type are -1, a spawner's item is the
/// null reference, and any other block's row is zero.
pub fn inert_row(block: BlockKind) -> Vec<u8> {
    let def = layout::block(block);
    let mut bytes = vec![0; def.stride as usize];
    match block {
        BlockKind::StartingLocations => bytes[0x2c..0x2e].fill(0xff),
        BlockKind::NetgameFlags => bytes[0x10..0x12].fill(0xff),
        BlockKind::NetgameEquipment => bytes[0x58..0x60].fill(0xff),
        _ if def.role(Role::PaletteIndex).is_some() => bytes[..4].fill(0xff),
        _ => {}
    }
    bytes
}

// ---- fields ----

/// A field's name as the layout spells it, for a step that holds a
/// `&'static str`.
pub fn field_def(block: BlockKind, name: &str) -> Result<&'static FieldDef, String> {
    let field = layout::block(block)
        .field(name)
        .ok_or_else(|| format!("{} has no field {name:?}", block.name()))?;
    if !field.editable {
        return Err(format!("{} {} is read-only", block.name(), field.name));
    }
    Ok(field)
}

fn group_word(group: &str) -> Result<u32, String> {
    let bytes: [u8; 4] = group
        .as_bytes()
        .try_into()
        .map_err(|_| format!("{group:?} is not a tag group"))?;
    // The cache stores the group as the big-endian word of its characters.
    Ok(u32::from_be_bytes(bytes))
}

fn fits(ty: FieldType, n: i64) -> bool {
    match ty {
        FieldType::U8 => u8::try_from(n).is_ok(),
        FieldType::I16 => i16::try_from(n).is_ok(),
        FieldType::U16 => u16::try_from(n).is_ok(),
        FieldType::U32 => u32::try_from(n).is_ok(),
        _ => false,
    }
}

/// A document value as a field of `block` stores it. `resolve` gives the
/// salted datum behind a tag key.
pub fn field_value(
    block: BlockKind,
    name: &str,
    value: &Value,
    mut resolve: impl FnMut(&TagKey) -> Result<u32, String>,
) -> Result<(&'static FieldDef, FieldValue), String> {
    let field = field_def(block, name)?;
    let wrong = || {
        format!(
            "{} {} wants another kind of value",
            block.name(),
            field.name
        )
    };
    let value = match (field.ty, field.role, value) {
        (_, Role::PaletteIndex, Value::Tag(key)) => FieldValue::Palette {
            datum: key.as_ref().map_or(Ok(NULL), &mut resolve)?,
        },
        (_, Role::PaletteIndex, _) => return Err(wrong()),
        (FieldType::TagRef, _, Value::Tag(None)) => FieldValue::Tag {
            group: NULL,
            datum: NULL,
        },
        (FieldType::TagRef, _, Value::Tag(Some(key))) => FieldValue::Tag {
            group: group_word(&key.group)?,
            datum: resolve(key)?,
        },
        (FieldType::F32, _, Value::Float(n)) => FieldValue::Float(n.get()),
        (FieldType::Vec3, _, Value::Vec3(v)) => FieldValue::Vec3(v.map(|c| c.get())),
        (ty, _, Value::Int(n)) if fits(ty, *n) => FieldValue::Int(*n),
        _ => return Err(wrong()),
    };
    Ok((field, value))
}

/// The bytes a field stores for `value`. A palette index is `index`, the
/// position of the selected entry, which the caller finds.
fn encode(field: &FieldDef, value: FieldValue, index: Option<i16>) -> Result<Vec<u8>, String> {
    let wrong = || format!("{} wants another kind of value", field.name);
    Ok(match (field.ty, value) {
        (_, FieldValue::Palette { .. }) => index.ok_or_else(wrong)?.to_le_bytes().to_vec(),
        (FieldType::U8, FieldValue::Int(n)) if fits(field.ty, n) => vec![n as u8],
        (FieldType::I16, FieldValue::Int(n)) if fits(field.ty, n) => {
            (n as i16).to_le_bytes().to_vec()
        }
        (FieldType::U16, FieldValue::Int(n)) if fits(field.ty, n) => {
            (n as u16).to_le_bytes().to_vec()
        }
        (FieldType::U32, FieldValue::Int(n)) if fits(field.ty, n) => {
            (n as u32).to_le_bytes().to_vec()
        }
        (FieldType::F32, FieldValue::Float(n)) if n.is_finite() => n.to_le_bytes().to_vec(),
        (FieldType::Vec3, FieldValue::Vec3(v)) if v.iter().all(|c| c.is_finite()) => {
            v.iter().flat_map(|c| c.to_le_bytes()).collect()
        }
        (FieldType::TagRef, FieldValue::Tag { group, datum }) => group
            .to_le_bytes()
            .into_iter()
            .chain(datum.to_le_bytes())
            .collect(),
        _ => return Err(wrong()),
    })
}

/// The position of the palette entry of `block` that holds `datum`, or -1
/// for the null datum.
fn palette_index(
    m: &dyn GuestMemory,
    scenario: u32,
    block: BlockKind,
    datum: u32,
) -> Result<i16, RpcError> {
    if datum == NULL {
        return Ok(-1);
    }
    let palette = layout::block(block)
        .palette
        .as_ref()
        .ok_or_else(|| invalid(format!("{} has no palette", block.name())))?;
    let count = m.u32(scenario + palette.offset).map_err(fault)?;
    let base = m.u32(scenario + palette.offset + 4).map_err(fault)?;
    for i in 0..count.min(i16::MAX as u32) {
        let at = base.wrapping_add(i * palette.stride);
        if m.u32(at + 4).map_err(fault)? == datum {
            return Ok(i as i16);
        }
    }
    Err(refused(format!(
        "the {} palette does not hold tag {datum:#010x}",
        block.name()
    )))
}

/// The row's position and orientation fields for a frame: the position, and
/// the rotation (yaw, pitch, roll) or the facing (the yaw) the block has.
fn frame_writes(block: BlockKind, frame: Frame) -> Result<Vec<(u32, Vec<u8>)>, String> {
    let def = layout::block(block);
    let position = def
        .role(Role::Position)
        .ok_or_else(|| format!("{} rows have no position", block.name()))?;
    let mut out = vec![(
        position.offset,
        encode(position, FieldValue::Vec3(frame.position), None)?,
    )];
    if let Some(rotation) = def.role(Role::Rotation) {
        out.push((
            rotation.offset,
            encode(rotation, FieldValue::Vec3(frame.rotation), None)?,
        ));
    } else if let Some(facing) = def.role(Role::Facing) {
        out.push((
            facing.offset,
            encode(facing, FieldValue::Float(frame.rotation[0]), None)?,
        ));
    }
    Ok(out)
}

// ---- the live object ----

/// The live object of an object block's row `index`: the one of the block's
/// type whose placement index is `index`. None when there is none (or the
/// block makes no objects). More than one, or one that is held or attached,
/// cannot be edited as the placement's.
pub fn live_object(
    m: &dyn GuestMemory,
    block: BlockKind,
    index: u32,
) -> Result<Option<ObjectId>, RpcError> {
    let Some((kind, ty)) = object_kind(block) else {
        return Ok(None);
    };
    let mut found = None;
    for obj in sim_state::objects(m).map_err(fault)? {
        let obj = obj.map_err(fault)?;
        if obj.kind().map_err(fault)? != Some(ty)
            || obj.flags().map_err(fault)?.deleting
            || obj.placement().map_err(fault)?.map(u32::from) != Some(index)
        {
            continue;
        }
        if obj.parent().map_err(fault)?.is_some() {
            return Err(refused(format!(
                "{OBJECT_HELD}: the {} {index} object is held or attached",
                block.name()
            )));
        }
        let id = ObjectId {
            kind,
            key: obj.id().0,
        };
        if found.replace(id).is_some() {
            return Err(refused(format!(
                "more than one live object occupies {} {index}",
                block.name()
            )));
        }
    }
    Ok(found)
}

// ---- the tick boundary ----

/// What [`plan`] found for one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The row's guest address.
    pub row: u32,
    /// The live object the step edits: for a move, the one that must move;
    /// for an inert row, the one to delete if there is one.
    pub object: Option<ObjectId>,
}

/// Check a step against the loaded scenario at `scenario` before anything
/// is written. The row must exist; an object block's move needs its live
/// object (`forge_object_gone` when it has none); a restore of an object
/// block needs no object already there.
pub fn plan(m: &dyn GuestMemory, scenario: u32, step: &BaseStep) -> Result<Plan, RpcError> {
    let (block, index) = (step.block(), step.index());
    let row = row_address(m, scenario, block, index)?;
    let object = match step {
        BaseStep::Move { frame, .. } => {
            frame_writes(block, *frame).map_err(invalid)?;
            let object = live_object(m, block, index)?;
            if object.is_none() && object_kind(block).is_some() {
                return Err(refused(format!(
                    "{OBJECT_GONE}: {} {index} has no live object",
                    block.name()
                )));
            }
            object
        }
        BaseStep::Field { field, value, .. } => {
            let def = field_def(block, field).map_err(invalid)?;
            let index = match value {
                FieldValue::Palette { datum } => Some(palette_index(m, scenario, block, *datum)?),
                _ => None,
            };
            encode(def, *value, index).map_err(invalid)?;
            None
        }
        BaseStep::Inert { .. } => live_object(m, block, index)?,
        BaseStep::Restore { row: kept, .. } => {
            let stride = layout::block(block).stride as usize;
            if kept.as_slice().len() != stride {
                return Err(invalid("the kept row is not this block's width"));
            }
            if let Some(held) = live_object(m, block, index)? {
                return Err(refused(format!(
                    "{} {index} already has a live object, {held}",
                    block.name()
                )));
            }
            None
        }
    };
    Ok(Plan { row, object })
}

/// Write a checked step's bytes into its row. The live object is the
/// caller's: it moves, deletes or makes it around this write.
pub fn write(
    m: &mut dyn GuestMemory,
    scenario: u32,
    step: &BaseStep,
    plan: Plan,
) -> Result<(), RpcError> {
    match step {
        BaseStep::Move { block, frame, .. } => {
            for (offset, bytes) in frame_writes(*block, *frame).map_err(invalid)? {
                m.write(plan.row + offset, &bytes).map_err(fault)?;
            }
        }
        BaseStep::Field {
            block,
            field,
            value,
            ..
        } => {
            let def = field_def(*block, field).map_err(invalid)?;
            let index = match value {
                FieldValue::Palette { datum } => Some(palette_index(m, scenario, *block, *datum)?),
                _ => None,
            };
            let bytes = encode(def, *value, index).map_err(invalid)?;
            m.write(plan.row + def.offset, &bytes).map_err(fault)?;
        }
        BaseStep::Inert { block, .. } => {
            m.write(plan.row, &inert_row(*block)).map_err(fault)?;
        }
        BaseStep::Restore { row, .. } => {
            m.write(plan.row, row.as_slice()).map_err(fault)?;
        }
    }
    Ok(())
}
