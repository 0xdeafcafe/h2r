//! Forge's tick-boundary probes (docs/game/forge-design.md, F3).

use std::collections::BTreeMap;

use agent_proto::forge::{CapacityReply, ProbeParams, ProbeReply, TagArena, method};
use agent_proto::sim::{OBJECT_GROUPS, SpawnParams, hashes::HashesParams};
use agent_proto::{Boundary, RpcError, code};
use guest_layout::{GuestMemory, MemFault};
use guest_layout::{game::GameGlobals, global};

use crate::sim::{SimHandle, hashes::HashLog};
use crate::{Access, Agent, GameCtx};

pub mod base;
mod capacity;
pub use capacity::{block_usage, object_pool};
pub mod ground;
pub mod local;
pub mod nav;
pub mod net;
pub mod pieces;
pub mod place_at_start;
pub mod spare_clips;
pub mod spawn_weight;
pub mod test;
pub mod waves;

const AI_GLOBALS: u32 = 0x004f_55d0;
// The low and high physical arena cursors (docs/editor/scenario-editor.md §6).
const TAG_LOW: u32 = 0x004e_642c;
const TAG_HIGH: u32 = 0x004e_6440;
// scnr's kind is distinct from the running options' game mode.
const SCENARIO_TYPE: u32 = 0x10;
// The simulation world's out-of-sync latch (docs/live/live-coop.md).
const DESYNC: u32 = 0x2c;

const HASH_ROWS_MAX: usize = 4096;

pub fn install(agent: &Agent, sim: SimHandle, hashes: &'static HashLog) {
    agent.add_game_method(method::CAPACITY, Access::Read, |value, ctx| {
        if !value.is_null() && !value.as_object().is_some_and(|fields| fields.is_empty()) {
            return Err(invalid("forge.capacity takes no parameters"));
        }
        loaded_tick(ctx)?;
        let reply = ctx.with_guest(read_capacity)?;
        Ok(serde_json::to_value(reply).expect("serialises"))
    });
    agent.add_game_method(method::PROBE, Access::Act, move |value, ctx| {
        let params = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        let reply = match params {
            ProbeParams::Replication { group, tag, pos } => {
                if !OBJECT_GROUPS.contains(&group.as_str()) {
                    return Err(invalid(format!("{group:?} is not an object tag group")));
                }
                let session = sim.replication_session(ctx)?;
                let tag = match tag {
                    Some(tag) => tag,
                    None => ctx.with_guest(|m| first_tag(m, &group))?,
                };
                let pos = match pos {
                    Some(pos) => pos,
                    None => ctx.with_guest(|m| capacity::probe_position(m))?,
                };
                return sim.replication_spawn(
                    SpawnParams {
                        tag,
                        pos,
                        yaw: None,
                    },
                    session,
                    group,
                    ctx,
                );
            }
            ProbeParams::Sync { from_tick, max } => {
                let session = sim.session_view(ctx);
                let (scenario_type, variant_engine, desync) = ctx.with_guest(sync_state)?;
                let max = max.unwrap_or(HASH_ROWS_MAX);
                if !(1..=HASH_ROWS_MAX).contains(&max) {
                    return Err(invalid(format!("max: 1 to {HASH_ROWS_MAX}")));
                }
                // Exclude rows from before the current map and after this
                // boundary. A missing tick has no comparable sample.
                let hashes = ctx
                    .at
                    .tick
                    .filter(|_| ctx.at.map_ticks > 0 && scenario_type == Some(1))
                    .and_then(|tick| u32::try_from(tick).ok())
                    .map(|tick| {
                        let first = tick.saturating_sub(
                            u32::try_from(ctx.at.map_ticks - 1).unwrap_or(u32::MAX),
                        );
                        hashes.reply_through(
                            &HashesParams {
                                from_tick: Some(from_tick.unwrap_or(0).max(first)),
                                max: Some(max),
                            },
                            tick,
                        )
                    })
                    .unwrap_or_default();
                ProbeReply::Sync {
                    session,
                    scenario_type,
                    variant_engine,
                    desync,
                    hashes,
                }
            }
            ProbeParams::Ai {} => {
                loaded_tick(ctx)?;
                ctx.with_guest(read_ai)?
            }
            ProbeParams::Kit {} => {
                loaded_tick(ctx)?;
                ctx.with_guest(read_kit)?
            }
        };
        Ok(serde_json::to_value(reply).expect("serialises"))
    });
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

fn fault(e: MemFault) -> RpcError {
    unavailable(format!("reading Forge state at {:#010x}", e.addr))
}

fn loaded_tick(ctx: &GameCtx<'_>) -> Result<(), RpcError> {
    if ctx.at.boundary != Boundary::Tick
        || !matches!(ctx.at.mode.as_deref(), Some("multiplayer" | "campaign"))
    {
        return Err(unavailable(
            "Forge measurement requires a loaded gameplay tick",
        ));
    }
    Ok(())
}

fn scenario(m: &dyn GuestMemory) -> Result<u32, RpcError> {
    global::SCENARIO
        .pointer(m)
        .map_err(fault)?
        .ok_or_else(|| unavailable("no loaded scenario"))
}

fn read_capacity(m: &mut dyn GuestMemory) -> Result<CapacityReply, RpcError> {
    let scenario = scenario(m)?;
    let low = m.u32(TAG_LOW).map_err(fault)?;
    let high = m.u32(TAG_HIGH).map_err(fault)?;
    let headroom_bytes = high
        .checked_sub(low)
        .filter(|_| low != 0)
        .ok_or_else(|| unavailable("invalid physical tag arena cursors"))?;
    Ok(CapacityReply {
        tag_arena: TagArena {
            low,
            high,
            headroom_bytes,
        },
        object_pool: capacity::object_pool(m)?,
        blocks: capacity::blocks(m, scenario)?,
    })
}

fn sync_state(
    m: &mut dyn GuestMemory,
) -> Result<(Option<u16>, Option<i32>, Option<bool>), RpcError> {
    let scenario_type = global::SCENARIO
        .pointer(m)
        .map_err(fault)?
        .map(|s| m.u16(s + SCENARIO_TYPE))
        .transpose()
        .map_err(fault)?;
    let variant_engine = GameGlobals::current(m)
        .map_err(fault)?
        .map(|g| g.options().variant().engine(m))
        .transpose()
        .map_err(fault)?;
    let desync = global::SIM_GLOBALS
        .pointer(m)
        .map_err(fault)?
        .map(|s| m.u8(s + DESYNC).map(|byte| byte != 0))
        .transpose()
        .map_err(fault)?;
    Ok((scenario_type, variant_engine, desync))
}

fn read_ai(m: &mut dyn GuestMemory) -> Result<ProbeReply, RpcError> {
    let ai = m.u32(AI_GLOBALS).map_err(fault)?;
    // The engine's ai builtin and ai_update access +0 (ai-control.md).
    // +1 is the separate activity byte (h1_ai_control::actors::active).
    let (enabled, active) = if ai == 0 {
        (None, None)
    } else {
        (
            Some(m.u8(ai).map_err(fault)? != 0),
            Some(m.u8(ai + 1).map_err(fault)? != 0),
        )
    };
    Ok(ProbeReply::Ai {
        enabled,
        active,
        actors: crate::ai::actor_count(m).map_err(fault)?,
    })
}

fn read_kit(m: &mut dyn GuestMemory) -> Result<ProbeReply, RpcError> {
    let blocks = capacity::blocks(m, scenario(m)?)?;
    // Seed zeroes too, so two census replies have the same keys.
    let mut objects_by_group: BTreeMap<_, u32> = OBJECT_GROUPS
        .iter()
        .map(|group| (group.to_string(), 0))
        .collect();
    objects_by_group.insert("unknown".into(), 0);
    for object in sim_state::objects(&*m).map_err(fault)? {
        let kind = object.map_err(fault)?.kind().map_err(fault)?;
        let group = kind.map_or("unknown", |kind| kind.group());
        *objects_by_group.entry(group.to_owned()).or_default() += 1;
    }
    Ok(ProbeReply::Kit {
        blocks,
        objects_by_group,
        actors: crate::ai::actor_count(m).map_err(fault)?,
    })
}

/// Choose the first valid loaded tag in index order when the caller only names a group.
fn first_tag(m: &dyn GuestMemory, group: &str) -> Result<String, RpcError> {
    use guest_layout::tags::{entry_layout as entry, index_layout as index};
    let wanted = u32::from_be_bytes(group.as_bytes().try_into().expect("validated object group"));
    let header = global::TAG_INDEX
        .pointer(m)
        .map_err(fault)?
        .ok_or_else(|| unavailable("no loaded tag index"))?;
    let table = global::TAG_ENTRIES
        .pointer(m)
        .map_err(fault)?
        .ok_or_else(|| unavailable("no loaded tag entries"))?;
    let count = m.u32(header + index::TAG_COUNT).map_err(fault)?;
    // tag_entry_checked sign-extends a tag's slot.
    if count > 0x8000 || table.checked_add(count * entry::STRIDE).is_none() {
        return Err(unavailable("invalid loaded tag index"));
    }
    for slot in 0..count {
        let row = table + slot * entry::STRIDE;
        if m.u32(row + entry::GROUP).map_err(fault)? != wanted
            || m.u32(row + entry::ADDRESS).map_err(fault)? == 0
        {
            continue;
        }
        let datum = m.u32(row + entry::DATUM).map_err(fault)?;
        if u32::from(datum as u16) == slot {
            return Ok(format!("{datum:#010x}"));
        }
    }
    Err(unavailable(format!("no loaded tag of group {group}")))
}

#[cfg(test)]
mod tests;
