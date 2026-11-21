//! Reserve trailing scenario slots without exposing them to the map's readers.

use std::io::{Cursor, Read, Seek};

use map::{Map, scenario_layout as layout};
use scenario_edit::layout::{BlockKind, Role};

use crate::Error;

const SPARES: u32 = 8;
const INDEX_LIMIT: u32 = i16::MAX as u32 + 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub block: &'static str,
    pub active: u32,
    pub spare: u32,
}

#[derive(Clone, Copy)]
enum Spare {
    Zero,
    Placement,
    Index(u32),
    Item,
}

#[derive(Clone, Copy)]
struct Block {
    name: &'static str,
    offset: u32,
    stride: u32,
    limit: u32,
    spare: Spare,
}

// Layouts match the Forge capacity probe's blocks (docs/game/forge-design.md).
const EXTRA: &[Block] = &[
    Block {
        name: "object_names",
        offset: layout::OBJECT_NAMES,
        stride: layout::OBJECT_NAME_STRIDE,
        limit: 640,
        spare: Spare::Index(0x22),
    },
    Block {
        name: "bipeds",
        offset: 0x60,
        stride: 0x54,
        limit: INDEX_LIMIT,
        spare: Spare::Placement,
    },
    Block {
        name: "device_groups",
        offset: 0xa0,
        stride: 0x28,
        limit: INDEX_LIMIT,
        spare: Spare::Zero,
    },
    Block {
        name: "controls",
        offset: 0xb8,
        stride: 0x44,
        limit: INDEX_LIMIT,
        spare: Spare::Placement,
    },
    Block {
        name: "light_fixtures",
        offset: 0xc8,
        stride: 0x54,
        limit: INDEX_LIMIT,
        spare: Spare::Placement,
    },
    Block {
        name: "squad_groups",
        offset: 0x158,
        stride: 0x24,
        limit: INDEX_LIMIT,
        spare: Spare::Zero,
    },
    Block {
        name: "squads",
        offset: 0x160,
        stride: 0x74,
        limit: INDEX_LIMIT,
        spare: Spare::Zero,
    },
    Block {
        name: "creatures",
        offset: 0x368,
        stride: 0x34,
        limit: INDEX_LIMIT,
        spare: Spare::Placement,
    },
];

fn blocks() -> Vec<Block> {
    let mut blocks: Vec<_> = layout::BLOCKS
        .iter()
        .map(|def| {
            let spare = match def.kind {
                BlockKind::StartingLocations => Spare::Index(0x2c),
                BlockKind::NetgameFlags => Spare::Index(0x10),
                BlockKind::NetgameEquipment => Spare::Item,
                _ if def.role(Role::PaletteIndex).is_some() => Spare::Placement,
                _ => Spare::Zero,
            };
            Block {
                name: def.kind.name(),
                offset: def.offset,
                stride: def.stride,
                limit: def.max_count.map_or(INDEX_LIMIT, |n| n as u32),
                spare,
            }
        })
        .collect();
    blocks.extend_from_slice(EXTRA);
    blocks.sort_unstable_by_key(|b| b.offset);
    blocks
}

impl Block {
    fn entry(&self) -> Vec<u8> {
        let mut bytes = vec![0; self.stride as usize];
        match self.spare {
            Spare::Zero => {}
            Spare::Placement => bytes[..4].fill(0xff), // palette and object-name indices
            Spare::Index(offset) => bytes[offset as usize..][..2].fill(0xff),
            Spare::Item => bytes[0x58..0x60].fill(0xff), // null group and datum
        }
        bytes
    }
}

pub(super) fn reserve(
    bytes: Vec<u8>,
    new_strings: &[String],
) -> Result<(Vec<u8>, Vec<Reservation>), Error> {
    let mut map = Map::from_reader(Cursor::new(bytes.as_slice()))?;
    let scenario = map.scenario().ok_or(Error::Kit("missing scenario"))?.id;
    let location = map
        .tag(scenario)?
        .location
        .ok_or(Error::Kit("scenario has no local payload"))?;
    let root = map.read_tag_bytes(scenario)?;
    let mut planned = Vec::new();
    for block in blocks() {
        let Some(pair) = root.get(block.offset as usize..block.offset as usize + 8) else {
            return Err(Error::Kit("scenario root too short for kit block"));
        };
        let count = word(&pair[..4]);
        let address = word(&pair[4..]);
        let spare = block.limit.saturating_sub(count).min(SPARES);
        if spare != 0 {
            let existing = read_entries(&mut map, address, count * block.stride)?;
            planned.push((block, count, spare, existing));
        }
    }

    let mut editor = map.edit();
    let mut reserves = Vec::new();
    for (block, active, mut spare, mut data) in planned {
        let entry = block.entry();
        let active_bytes = data.len();
        data.extend(entry.iter().cycle().take(spare as usize * entry.len()));
        while spare > 0 {
            let len = active_bytes + spare as usize * entry.len();
            match editor.append_metadata(&data[..len], 16) {
                Ok(pointer) => {
                    // The root count stays at the number of active entries.
                    editor.patch_metadata(
                        location.address + block.offset + 4,
                        &pointer.to_le_bytes(),
                    )?;
                    reserves.push(Reservation {
                        block: block.name,
                        active,
                        spare,
                    });
                    break;
                }
                Err(map::Error::Invalid(
                    "grown metadata exceeds the engine's tag memory"
                    | "edited map exceeds engine cache size limit",
                )) => spare -= 1,
                Err(error) => return Err(error.into()),
            }
        }
    }

    if reserves.is_empty() && new_strings.is_empty() {
        return Ok((bytes, reserves));
    }
    for text in new_strings {
        editor.add_string(text)?;
    }
    let mut output = Cursor::new(Vec::new());
    editor.write_to(&mut output)?;
    Ok((output.into_inner(), reserves))
}

fn read_entries<R: Read + Seek>(
    map: &mut Map<R>,
    address: u32,
    size: u32,
) -> Result<Vec<u8>, Error> {
    if size == 0 {
        return Ok(Vec::new());
    }
    let offset = map.file_offset(address, size)?;
    Ok(map.read_file_range(offset, size.into())?)
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("one scenario block word"))
}
