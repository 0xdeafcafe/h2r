//! The kit's rows in a session (docs/game/forge-design.md, "The kit";
//! docs/game/forge-editing.md, "The object menu").
//!
//! A map materialised with the `forge_kit` recipe has spare rows after the
//! active entries of the starting location, netgame flag and netgame
//! equipment blocks, in the form [`inert_row`] gives. A kit session adds an
//! entry by writing the next spare row and raising the block's root count,
//! and takes it away by writing the spare form back and lowering the count
//! (`World::kit_add`, `World::kit_drop`). A map without the kit has no
//! spare row, so those adds are refused by name ([`no_kit`]).
use forge_doc::{BlockKind, TagKey, Value};
use map::scenario_layout::{self as layout, FieldDef, FieldType, Role};

/// The blocks a kit session adds entries to.
pub const BLOCKS: [BlockKind; 3] = [
    BlockKind::StartingLocations,
    BlockKind::NetgameFlags,
    BlockKind::NetgameEquipment,
];

/// The most spare rows the kit reserves in a block
/// (`forge-build`'s `kit.rs`).
pub const MAX_SPARE: u32 = 8;

/// What a refusal for a map with no kit begins with
/// (`agent_proto::forge::refusal::NO_KIT`).
pub const NO_KIT: &str = "This map has no kit";

/// Whether a kit session can add entries to `block`.
pub fn is_block(block: BlockKind) -> bool {
    BLOCKS.contains(&block)
}

/// The sentence of an add to a map that has no kit.
pub fn no_kit(block: BlockKind) -> String {
    let (one, many) = crate::noun(block);
    format!(
        "{NO_KIT}, so it has no room for another {one}. \
         Start a new map in Forge mode to add {many}."
    )
}

/// The sentence of an add to a kit whose spare rows of `block` are used.
pub fn full(block: BlockKind) -> String {
    let (one, _) = crate::noun(block);
    format!("The kit has no room for another {one}.")
}

/// The spare form of a row of `block`: the bytes the Forge kit gives its
/// reserved rows, which the engine's readers skip (`forge-build`'s
/// `kit.rs`, `Block::entry`). A placement's palette and object-name indices
/// are -1, a starting point's player type and a flag's type are -1, a
/// spawner's item is the null reference, and any other block's row is zero.
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

/// How many spare rows follow the `count` active rows of `block`: the run of
/// rows, from the first past the count, that `read` gives in the spare
/// form, at most [`MAX_SPARE`] and what the engine's count for the block
/// leaves. `read` answers the bytes of row `n`, or none where the map has no
/// such row.
pub fn room(block: BlockKind, count: u32, mut read: impl FnMut(u32) -> Option<Vec<u8>>) -> u32 {
    let limit = layout::block(block)
        .max_count
        .map_or(MAX_SPARE, |max| (max as u32).saturating_sub(count).min(MAX_SPARE));
    let spare = inert_row(block);
    (0..limit)
        .take_while(|k| read(count + k).is_some_and(|row| row == spare))
        .count() as u32
}

/// The value a field has in a zeroed row: what the materialiser's new
/// entry holds until an add or a set writes it.
pub(crate) fn zero(field: &FieldDef) -> Value {
    match field.ty {
        FieldType::U8 | FieldType::I16 | FieldType::U16 | FieldType::U32 => Value::Int(0),
        FieldType::F32 => Value::Float(forge_doc::F32::new(0.0).expect("zero is finite")),
        FieldType::Vec3 => {
            let zero = forge_doc::F32::new(0.0).expect("zero is finite");
            Value::Vec3([zero; 3])
        }
        FieldType::TagRef => Value::Tag(None),
    }
}

/// The group of the object menu an item spawner's item is offered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemGroup {
    Weapons,
    Vehicles,
    Equipment,
}

/// The group of `tag`, an item or vehicle collection a spawner of the loaded
/// map names: a vehicle collection is a vehicle, a collection whose path
/// names powerups or equipment is equipment, any other is a weapon.
pub fn item_group(tag: &TagKey) -> ItemGroup {
    let path = tag.path.to_ascii_lowercase();
    if tag.group == "vehc" || path.contains("vehicle") {
        ItemGroup::Vehicles
    } else if path.contains("powerup") || path.contains("equipment") {
        ItemGroup::Equipment
    } else {
        ItemGroup::Weapons
    }
}
