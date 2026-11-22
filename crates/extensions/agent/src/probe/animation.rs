//! Use the same lookup and inheritance resolution as the animation manager.
use crate::sim::calls::Outcome;
use agent_proto::{RpcError, code};
use engine::{
    DatumHandle, GuestMemory, MemFault,
    entry::{Cx, EntryError, GuestCall},
    view::{Tags, objects},
};
use live_telemetry::layout as l;
use serde_json::json;

fn unavailable(why: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, why)
}

fn graph(m: &dyn GuestMemory, unit: u32) -> Result<Option<(u32, u32)>, MemFault> {
    let Some((objects::BIPED, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(None);
    };
    let offset = m.u16(body.addr + l::OBJECT_ANIMATION_MANAGER)? as i16;
    if !l::MANAGER_OFFSETS.contains(&offset) {
        return Ok(None);
    }
    let graph = m.u32(body.addr + offset as u32 + l::MANAGER_GRAPH)?;
    let root = Tags.get(
        m,
        DatumHandle(graph),
        engine::tags::GroupTag::from_chars(*b"jmad"),
    )?;
    Ok(root.map(|r| (graph, r)))
}

pub(crate) async fn lookup(
    cx: &Cx,
    scratch: u32,
    unit: u32,
    path: [u32; 4],
    overlay: bool,
) -> Result<Outcome, EntryError> {
    let Some((datum, root)) = cx.read(|m| graph(m, unit))? else {
        return Ok(Err(unavailable(format!(
            "unit {unit:08x} has no live biped animation graph"
        ))));
    };
    let [mode, class, kind, action] = path;
    // 0x1cb2a5 and 0x1cb2c2 pass the graph, output pair, mode/class/type
    // and optional fallback outputs. EAX is the requested action SID.
    cx.write(|m| m.write(scratch, &[0xff; 16]))?;
    cx.call(
        GuestCall::new(if overlay { 0x1dbc80 } else { 0x1db170 })
            .eax(action)
            .args([
                root,
                scratch,
                mode,
                class,
                kind,
                scratch + 4,
                scratch + 8,
                scratch + 12,
            ]),
    )
    .await?;
    let pair = cx.read(|m| m.u32(scratch))?;
    let index = (pair >> 16) as i16;
    if index < 0 {
        return Ok(Err(unavailable(format!(
            "graph {datum:08x} has no entry for {path:?}"
        ))));
    }
    let entry = cx
        .call(GuestCall::new(0x1daea0).edx(root).args([pair]))
        .await?
        .eax;
    if entry == 0 {
        return Ok(Err(unavailable("the animation entry did not resolve")));
    }
    let (name, frames) = cx.read(|m| Ok::<_, MemFault>((m.u32(entry)?, m.u16(entry + 0x14)?)))?;
    let selected = cx.read(|m| {
        Ok::<_, MemFault>([
            m.u32(scratch + 4)?,
            m.u32(scratch + 8)?,
            m.u32(scratch + 12)?,
        ])
    })?;
    Ok(Ok(
        json!({"graph": datum, "path": path, "selected":selected, "inherited": pair as i16,
        "index": index, "animation": name, "frames": frames}),
    ))
}
