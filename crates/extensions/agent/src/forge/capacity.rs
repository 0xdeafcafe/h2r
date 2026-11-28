//! The scenario blocks a Forge kit can reserve.

use std::collections::BTreeMap;

use agent_proto::RpcError;
use agent_proto::forge::{BlockCapacity, ObjectPool};
use guest_layout::{GuestMemory, datum::{DatumArray, layout}};
use guest_layout::global;

use super::{fault, scenario, unavailable};

#[derive(Clone, Copy)]
enum Marker {
    I16(u32),
    NullTag(u32),
    Unknown,
}

struct Block {
    name: &'static str,
    offset: u32,
    stride: u32,
    limit: Option<u32>,
    marker: Marker,
}

const fn placement(name: &'static str, offset: u32, stride: u32, limit: Option<u32>) -> Block {
    Block {
        name,
        offset,
        stride,
        limit,
        marker: Marker::I16(0),
    }
}

const STARTS: Block = Block {
    name: "starting_locations",
    offset: 0x100,
    stride: 0x34,
    limit: None,
    marker: Marker::I16(0x2c),
};

// Retail strides from docs/formats/objects.md and docs/editor/scenario-editor.md.
// Device groups: docs/halo1/scripts.md. Squad blocks: docs/halo1/ai.md.
const BLOCKS: &[Block] = &[
    Block {
        name: "object_names",
        offset: 0x48,
        stride: 0x24,
        limit: Some(640),
        marker: Marker::I16(0x22),
    },
    placement("scenery", 0x50, 0x5c, Some(2000)),
    placement("bipeds", 0x60, 0x54, None),
    placement("vehicles", 0x70, 0x54, Some(256)),
    placement("equipment", 0x80, 0x38, Some(256)),
    placement("weapons", 0x90, 0x54, Some(128)),
    Block {
        name: "device_groups",
        offset: 0xa0,
        stride: 0x28,
        limit: None,
        marker: Marker::Unknown,
    },
    placement("machines", 0xa8, 0x48, Some(400)),
    placement("controls", 0xb8, 0x44, None),
    placement("light_fixtures", 0xc8, 0x54, None),
    placement("sound_scenery", 0xd8, 0x50, Some(256)),
    STARTS,
    Block {
        name: "netgame_flags",
        offset: 0x118,
        stride: 0x20,
        limit: None,
        marker: Marker::I16(0x10),
    },
    Block {
        name: "netgame_equipment",
        offset: 0x120,
        stride: 0x90,
        limit: Some(100),
        marker: Marker::NullTag(0x5c),
    },
    Block {
        name: "starting_equipment",
        offset: 0x128,
        stride: 0x9c,
        limit: None,
        marker: Marker::Unknown,
    },
    Block {
        name: "squad_groups",
        offset: 0x158,
        stride: 0x24,
        limit: None,
        marker: Marker::Unknown,
    },
    Block {
        name: "squads",
        offset: 0x160,
        stride: 0x74,
        limit: None,
        marker: Marker::Unknown,
    },
    placement("crates", 0x328, 0x4c, Some(1024)),
    placement("creatures", 0x368, 0x34, None),
];

impl Block {
    fn extent(&self, m: &dyn GuestMemory, scenario: u32) -> Result<(u32, u32), RpcError> {
        let pointer = scenario
            .checked_add(self.offset + 4)
            .ok_or_else(|| unavailable("invalid scenario address"))?;
        let count = m.u32(pointer - 4).map_err(fault)?;
        if count == 0 {
            return Ok((0, 0));
        }
        let address = m.u32(pointer).map_err(fault)?;
        if count > u32::from(u16::MAX)
            || address == 0
            || address.checked_add(count * self.stride).is_none()
        {
            return Err(unavailable(format!("invalid {} block extent", self.name)));
        }
        Ok((count, address))
    }

    fn read(&self, m: &dyn GuestMemory, scenario: u32) -> Result<BlockCapacity, RpcError> {
        let (count, address) = self.extent(m, scenario)?;
        let kit_left = match self.marker {
            Marker::Unknown => None,
            marker => {
                let mut tail = 0;
                for i in (0..count).rev() {
                    let row = address + i * self.stride;
                    let spare = match marker {
                        Marker::I16(offset) => m.u16(row + offset).map_err(fault)? == u16::MAX,
                        Marker::NullTag(offset) => m.u32(row + offset).map_err(fault)? == u32::MAX,
                        Marker::Unknown => unreachable!(),
                    };
                    if !spare {
                        break;
                    }
                    tail += 1;
                }
                Some(tail)
            }
        };
        Ok(BlockCapacity {
            count,
            limit: self.limit(m)?,
            kit_left,
        })
    }

    fn limit(&self, m: &dyn GuestMemory) -> Result<Option<u32>, RpcError> {
        Ok(match self.name {
            // The optional campaign extension changes the squad header's
            // capacity at load. Read the actual allocated array here.
            "squads" => pool(m, 0x0051_e9d8)?.map(|p| p.limit),
            "squad_groups" => pool(m, 0x0051_e9dc)?.map(|p| p.limit),
            _ => self.limit,
        })
    }
}

/// A scenario block's live count and known engine limit. This shares the
/// probe's measurements without scanning the block for spare kit rows.
pub fn block_usage(
    m: &dyn GuestMemory,
    scenario: u32,
    name: &str,
) -> Result<(u32, Option<u32>), RpcError> {
    let block = BLOCKS
        .iter()
        .find(|b| b.name == name)
        .ok_or_else(|| unavailable(format!("unknown scenario block {name}")))?;
    let (count, _) = block.extent(m, scenario)?;
    Ok((count, block.limit(m)?))
}

pub(super) fn blocks(
    m: &dyn GuestMemory,
    scenario: u32,
) -> Result<BTreeMap<String, BlockCapacity>, RpcError> {
    BLOCKS
        .iter()
        .map(|b| Ok((b.name.to_owned(), b.read(m, scenario)?)))
        .collect()
}

fn pool(m: &dyn GuestMemory, global: u32) -> Result<Option<ObjectPool>, RpcError> {
    let address = m.u32(global).map_err(fault)?;
    if address == 0 {
        return Ok(None);
    }
    let array = DatumArray::at(address);
    let used = array.header_field(m, layout::ACTIVE_COUNT).map_err(fault)?;
    let limit = array
        .header_field(m, layout::MAXIMUM_COUNT)
        .map_err(fault)?;
    if used < 0 || limit <= 0 || used > limit {
        return Err(unavailable(format!(
            "invalid datum pool at {address:#010x}"
        )));
    }
    Ok(Some(ObjectPool {
        used: used as u32,
        limit: limit as u32,
    }))
}

/// Current number of live object datums and the engine pool limit.
pub fn object_pool(m: &dyn GuestMemory) -> Result<ObjectPool, RpcError> {
    pool(m, global::OBJECTS.base)?.ok_or_else(|| unavailable("no active object pool"))
}

pub(super) fn probe_position(m: &dyn GuestMemory) -> Result<[f64; 3], RpcError> {
    let (count, address) = STARTS.extent(m, scenario(m)?)?;
    for i in 0..count {
        let row = address + i * STARTS.stride;
        if m.u16(row + 0x2c).map_err(fault)? == u16::MAX {
            continue;
        }
        return Ok([
            f64::from(m.f32(row).map_err(fault)?),
            f64::from(m.f32(row + 4).map_err(fault)?),
            f64::from(m.f32(row + 8).map_err(fault)?) + 1.0,
        ]);
    }
    Err(unavailable("no eligible starting location, supply pos"))
}
