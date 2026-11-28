//! Scenario scripts and globals at the interpreter's current tick boundary.
use agent_proto::{RpcError, code};
use engine::{GuestMemory, MemFault, datum::{DatumArray, layout}, points};
use serde_json::{Value, json};

use crate::{Access, Agent};

use engine::hs_actions;
use engine::hs_pool::{EXTERNALS, GLOBALS as CELLS, THREADS};
// hs_new_map at 0x20915f offsets scenario globals past the external cells.
const EXTERNAL_CELLS: usize = EXTERNALS as usize;
const KINDS: [&str; 6] = ["startup", "dormant", "continuous", "static", "stub", "command"];

pub fn install(agent: &Agent) {
    agent.add_game_method("hs.threads", Access::Read, |_, ctx| ctx.with_guest(|m| threads(m)));
    agent.add_game_method("hs.player_actions", Access::Read, |_, ctx| ctx.with_guest(|m| player_actions(m)));
    agent.add_game_method("hs.globals", Access::Read, |_, ctx| ctx.with_guest(|m| globals(m)));
    crate::hs_eval::install(agent);
}

fn unavailable(message: &str) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

fn fault(e: MemFault) -> RpcError {
    unavailable(&format!("HS state unreadable at {:#010x}", e.addr))
}

fn pointer(m: &dyn GuestMemory, at: u32) -> Result<u32, RpcError> {
    let p = m.u32(at).map_err(fault)?;
    if p == 0 { Err(unavailable("HS state has not been initialized")) } else { Ok(p) }
}

fn player_actions(m: &dyn GuestMemory) -> Result<Value, RpcError> {
    let control = pointer(m, hs_actions::PLAYER_CONTROL)?;
    if control.checked_add(hs_actions::UNTIL_RELEASE + 3).is_none() {
        return Err(unavailable("invalid player-control address"));
    }
    Ok(json!({
        "latched": m.u32(control + hs_actions::LATCHED).map_err(fault)?,
        "capture": m.u32(control + hs_actions::CAPTURE).map_err(fault)?,
        "until_release": m.u32(control + hs_actions::UNTIL_RELEASE).map_err(fault)?,
    }))
}

fn definitions(m: &dyn GuestMemory, offset: u32) -> Result<(usize, u32), RpcError> {
    let at = pointer(m, points::SCENARIO.base)?.checked_add(offset)
        .ok_or_else(|| unavailable("invalid scenario address"))?;
    let count = m.u32(at).map_err(fault)? as usize;
    let address = at.checked_add(4).ok_or_else(|| unavailable("invalid scenario address"))?;
    let defs = m.u32(address).map_err(fault)?;
    if count > 0x8000 || (count > 0 && (defs == 0 || defs.checked_add(count as u32 * 40).is_none())) {
        return Err(unavailable("invalid HS definition table"));
    }
    Ok((count, defs))
}

fn name(m: &dyn GuestMemory, at: u32) -> Result<String, RpcError> {
    let mut bytes = [0; 32];
    m.read(at, &mut bytes).map_err(fault)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn threads(m: &dyn GuestMemory) -> Result<Value, RpcError> {
    let (count, defs) = definitions(m, 0x1b8)?;
    let array = DatumArray::at(pointer(m, THREADS)?);
    let used = array.header_field(m, layout::HIGH_WATER).map_err(fault)?;
    if !(0..=320).contains(&used) { return Err(unavailable("invalid HS thread count")); }
    let mut out = Vec::new();
    for slot in 0..used {
        let Some(at) = array.get_absolute(m, slot).map_err(fault)? else { continue };
        let category = m.u8(at + 2).map_err(fault)?;
        let index = m.u16(at + 4).map_err(fault)? as usize;
        let (script, kind) = if matches!(category, 0 | 4) && index < count {
            let def = defs + index as u32 * 40;
            let kind = m.u16(def + 32).map_err(fault)? as usize;
            (Some(name(m, def)?), KINDS.get(kind))
        } else {
            (None, None)
        };
        out.push(json!({"slot":slot,"category":category,"script":script,"kind":kind,
                        "wake":m.i32(at + 8).map_err(fault)?}));
    }
    Ok(json!({"threads":out}))
}

fn globals(m: &dyn GuestMemory) -> Result<Value, RpcError> {
    let (count, defs) = definitions(m, 0x1c0)?;
    let array = DatumArray::at(pointer(m, CELLS)?);
    if array.header_field(m, layout::MAXIMUM_COUNT).map_err(fault)? < (count + EXTERNAL_CELLS) as i32 {
        return Err(unavailable("invalid HS global count"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let def = defs + i as u32 * 40;
        let cell = array.get_absolute(m, (EXTERNAL_CELLS + i) as i32).map_err(fault)?
            .ok_or_else(|| unavailable("scenario global has no live cell"))?;
        let bits = m.u32(cell + 4).map_err(fault)?;
        let ty = m.u16(def + 32).map_err(fault)?;
        let mut value = json!({"bits": bits});
        let label = match ty {
            5 => { value["boolean"] = json!(bits as u8 != 0); "boolean" }
            6 => { value["real"] = json!(f32::from_bits(bits)); "real" }
            7 => { value["number"] = json!(bits as i16); "short" }
            8 => { value["number"] = json!(bits as i32); "long" }
            9 => "string",
            10 => "script",
            11 => "string_id",
            _ => "other",
        };
        out.push(json!({"name":name(m, def)?,"type":label,"type_index":ty,"value":value}));
    }
    Ok(json!({"globals":out}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn player_actions_distinguish_a_press_from_a_script_claim() {
        let mut m = FakeGuest::new();
        m.put_u32(hs_actions::PLAYER_CONTROL, 0);
        assert!(player_actions(&m).is_err());
        m.put_u32(hs_actions::PLAYER_CONTROL, 0x1000)
            .put_u32(0x1004, 0x401).put_u32(0x1008, 0).put_u32(0x100c, 1);
        assert_eq!(player_actions(&m).unwrap(),
                   json!({"latched":0x401, "capture":0, "until_release":1}));
    }

    #[test]
    fn scenario_globals_follow_external_cells_and_keep_signed_values() {
        let mut m = FakeGuest::new();
        let index = EXTERNAL_CELLS as u32;
        m.put_u32(points::SCENARIO.base, 0x1000).put_u32(0x11c0, 1).put_u32(0x11c4, 0x2000)
            .put(0x2000, &[0;32]).put(0x2000, b"timer").put_u16(0x2020, 7)
            .put_u32(0x3000 + layout::MAXIMUM_COUNT, 3072)
            .put_u32(CELLS, 0x3000).put_u32(0x3000 + layout::ELEMENT_SIZE, 8)
            .put_u32(0x3000 + layout::HIGH_WATER, index + 1).put_u32(0x3000 + layout::DATA, 0x4000)
            .put_u16(0x4000 + 8 * index, 1).put_u32(0x4004 + 8 * index, 0xfffe);
        let v = globals(&m).unwrap();
        assert_eq!(v["globals"][0]["name"], "timer");
        assert_eq!(v["globals"][0]["value"], json!({"bits":65534,"number":-2}));
    }

    #[test]
    fn live_script_threads_use_script_metadata_and_signed_wake() {
        let mut m = FakeGuest::new();
        m.put_u32(points::SCENARIO.base, 0x1000).put_u32(0x11b8, 1).put_u32(0x11bc, 0x2000)
            .put(0x2000, &[0;32]).put(0x2000, b"wave").put_u16(0x2020, 1)
            .put_u32(THREADS, 0x3000).put_u32(0x3000 + layout::ELEMENT_SIZE, 0x418)
            .put_u32(0x3000 + layout::HIGH_WATER, 2).put_u32(0x3000 + layout::DATA, 0x4000)
            .put_u16(0x4000, 0).put_u16(0x4418, 8).put_u8(0x441a, 0)
            .put_u16(0x441c, 0).put_u32(0x4420, (-2i32) as u32);
        let v = threads(&m).unwrap();
        assert_eq!(v["threads"], json!([{"slot":1,"category":0,"script":"wave","kind":"dormant","wake":-2}]));
    }
}
