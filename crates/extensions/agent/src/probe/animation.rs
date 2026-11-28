//! Use the same lookup and inheritance resolution as the animation manager.
use crate::sim::calls::Outcome;
use agent_proto::{RpcError, code};
use guest_layout::{
    DatumHandle,
    GuestMemory,
    MemFault,
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
        guest_layout::tags::GroupTag::from_chars(*b"jmad"),
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

/// A channel's playback rate, `f32`: the frame setter advances the frame
/// by the tick length times it (docs/runtime/agent-api.md,
/// `world.object.animate`).
const CHANNEL_RATE: u32 = 0x18;
/// The jmad root's animation block: a count (`i32`) and a pointer.
const ANIMATIONS: u32 = 0x2c;
/// One animation entry: its name's string ID at `+0`, its frame count
/// (`u16`) at `+0x14`.
const ANIMATION_SIZE: u32 = 0x6c;

/// `world.object.animate`: point the object's first channel at the
/// animation called `name` in the graph the manager holds, at frame 0 and
/// rate 1. The graph's own animations only (an inherited one has no entry
/// here), so the channel's graph word stays -1, the channel's graph. All
/// of it is read before the first write.
pub(crate) fn start(
    m: &mut dyn GuestMemory,
    body: u32,
    name: u32,
) -> Result<serde_json::Value, RpcError> {
    let fault = |e: MemFault| unavailable(format!("guest memory fault at {:#x}", e.addr));
    let offset = m.u16(body + l::OBJECT_ANIMATION_MANAGER).map_err(fault)? as i16;
    if !l::MANAGER_OFFSETS.contains(&offset) {
        return Err(unavailable("the object has no animation manager"));
    }
    let manager = body + offset as u32;
    let graph = m.u32(manager + l::MANAGER_GRAPH).map_err(fault)?;
    let root = Tags
        .get(
            m,
            DatumHandle(graph),
            guest_layout::tags::GroupTag::from_chars(*b"jmad"),
        )
        .map_err(fault)?
        .ok_or_else(|| unavailable(format!("the object's graph {graph:08x} is not loaded")))?;
    let count = m.i32(root + ANIMATIONS).map_err(fault)?;
    let first = m.u32(root + ANIMATIONS + 4).map_err(fault)?;
    if !(0..=i16::MAX as i32).contains(&count) || (count > 0 && first == 0) {
        return Err(unavailable("the graph's animation block is unreadable"));
    }
    let mut found = None;
    for index in 0..count as u32 {
        let entry = first.wrapping_add(index.wrapping_mul(ANIMATION_SIZE));
        if m.u32(entry).map_err(fault)? == name {
            found = Some((index, m.u16(entry + 0x14).map_err(fault)?));
            break;
        }
    }
    let Some((index, frames)) = found else {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("graph {graph:08x} holds no animation with string ID {name:#x}"),
        ));
    };
    let channel = manager + l::MANAGER_CHANNEL;
    m.set_u16(channel + l::CHANNEL_ANIMATION, u16::MAX)
        .map_err(fault)?;
    m.set_u16(channel + l::CHANNEL_ANIMATION + 2, index as u16)
        .map_err(fault)?;
    m.set_f32(channel + l::CHANNEL_FRAME, 0.0).map_err(fault)?;
    m.set_f32(channel + CHANNEL_RATE, 1.0).map_err(fault)?;
    Ok(json!({"graph": graph, "animation": name, "index": index, "frames": frames, "frame": 0.0}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::tags::{GroupTag, TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL};
    use guest_layout::testing::FakeGuest;

    const BODY: u32 = 0x8020_0000;

    /// A scenery-like body whose manager (at +0x200) holds graph `0x45670000`,
    /// a jmad of two animations (string IDs 11 and 22).
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(BODY, 0x400);
        g.put_u16(BODY + l::OBJECT_ANIMATION_MANAGER, 0x200)
            .put_u32(BODY + 0x200 + l::MANAGER_GRAPH, 0x4567_0000)
            .put_u32(TAG_INDEX_GLOBAL, 0x6000)
            .put_u32(TAG_ENTRIES_GLOBAL, 0x7000)
            .put_u32(0x6000, 0x8000)
            .put_u32(0x6004, 1)
            .put_u32(0x6018, 1)
            .put_u32(0x7000, GroupTag::from_chars(*b"jmad").0)
            .put_u32(0x7004, 0x4567_0000)
            .put_u32(0x7008, 0x9000)
            .put_u32(0x8000, GroupTag::from_chars(*b"jmad").0)
            .put_u32(0x9000 + ANIMATIONS, 2)
            .put_u32(0x9000 + ANIMATIONS + 4, 0xa000)
            .put_u32(0xa000, 11)
            .put_u16(0xa014, 20)
            .put_u32(0xa000 + ANIMATION_SIZE, 22)
            .put_u16(0xa000 + ANIMATION_SIZE + 0x14, 31);
        g
    }

    #[test]
    fn start_points_the_first_channel_at_the_named_animation() {
        let mut g = guest();
        let out = start(&mut g, BODY, 22).unwrap();
        assert_eq!(
            (out["index"].clone(), out["frames"].clone()),
            (json!(1), json!(31))
        );
        let channel = BODY + 0x200 + l::MANAGER_CHANNEL;
        assert_eq!(g.u16(channel + l::CHANNEL_ANIMATION).unwrap(), u16::MAX);
        assert_eq!(g.u16(channel + l::CHANNEL_ANIMATION + 2).unwrap(), 1);
        assert_eq!(g.f32(channel + l::CHANNEL_FRAME).unwrap(), 0.0);
        assert_eq!(g.f32(channel + CHANNEL_RATE).unwrap(), 1.0);
    }

    #[test]
    fn start_refuses_an_unknown_name_and_a_body_without_a_manager_leaving_the_channel() {
        let mut g = guest();
        let e = start(&mut g, BODY, 99).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        let channel = BODY + 0x200 + l::MANAGER_CHANNEL;
        assert_eq!(g.u16(channel + l::CHANNEL_ANIMATION + 2).unwrap(), 0);
        g.put_u16(BODY + l::OBJECT_ANIMATION_MANAGER, u16::MAX);
        assert_eq!(start(&mut g, BODY, 11).unwrap_err().code, code::UNAVAILABLE);
    }
}
