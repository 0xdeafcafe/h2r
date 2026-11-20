//! Read the actor's decisions at the same tick boundary as world.query.

use agent_proto::{Kind, ObjectId, RpcError, code};
use engine::{GuestMemory, MemFault, view::Datums};
use serde_json::{Value, json};

use crate::{Access, Agent};

// Actor pool creation at 0x001dfb24 (256 records, stride 0x888).
const ACTORS: u32 = 0x004f55f0;
const ACTIVE_BSP: u32 = 0x004e0348;
const PROP_REFS: u32 = 0x00502418;
// The retail behavior descriptors and their parallel name table. Slots
// 0..=130 have names. The names themselves come from the loaded image.
const BEHAVIOR_NAMES: u32 = 0x0046f0c8;
const NAMED_BEHAVIORS: u16 = 131;

engine::record! {
    // 0x001b368a follows actor +0x338 through this array, then uses
    // the object handle at +0x20 to read the target's object body.
    struct PropRef: 0x3c {
        object @ 0x20: u32,
    }
}

engine::record! {
    struct Actor: 0x888 {
        // 0x001e0dab also installs the reciprocal actor handle in the unit.
        unit @ 0x18: u32,
        team @ 0x24: i16,
        // Squad assignment at 0x00203e84, scenario squad index.
        squad @ 0x30: i32,
        character @ 0x54: u32,
        // ai_combat_status takes the maximum of these (0x00276070).
        combat_status @ 0x86: i16,
        // 0x001a813d reads the top of four 64-byte behavior frames.
        behavior_depth @ 0x190: i16,
        // 0x0026c19d checks the cache before refreshing position and sector.
        location_cached @ 0x278: u8,
        sector @ 0x28c: i32,
        focus @ 0x338: Option<engine::handle::DatumHandle>,
    }
}

fn sector_count(m: &dyn GuestMemory) -> Result<i32, MemFault> {
    let bsp = m.u32(ACTIVE_BSP)?;
    if bsp == 0 || m.i32(bsp + 0xc4)? != 1 {
        return Ok(0);
    }
    let mesh = m.u32(bsp + 0xc8)?;
    if mesh == 0 { Ok(0) } else { m.i32(mesh) }
}

fn snapshot(m: &dyn GuestMemory) -> Result<Vec<Value>, MemFault> {
    let Some(pool) = Datums::<Actor>::from_global(m, ACTORS)? else {
        return Ok(Vec::new());
    };
    let props = Datums::<PropRef>::from_global(m, PROP_REFS)?;
    let count = sector_count(m)?;
    let mut actors = Vec::new();
    for (handle, a) in pool.live(m, 256)? {
        let depth = a.behavior_depth(m)?;
        let behavior = if (0..4).contains(&depth) {
            Some(m.u16(a.addr + 0x90 + 0x40 * depth as u32)?)
        } else {
            None
        };
        let behavior = behavior.filter(|&v| v != u16::MAX);
        let behavior_name = if let Some(id) = behavior.filter(|&id| id < NAMED_BEHAVIORS) {
            let name = m.u32(BEHAVIOR_NAMES + 4 * u32::from(id))?;
            if name == 0 {
                None
            } else {
                Some(String::from_utf8_lossy(&m.cstr(name, 96)?).into_owned())
            }
        } else {
            None
        };
        let unit = a.unit(m)?;
        let character = a.character(m)?;
        let squad = a.squad(m)?;
        let cached = a.location_cached(m)? != 0;
        let sector = a.sector(m)?;
        let focus = match (props, a.focus(m)?) {
            (Some(refs), Some(id)) => refs.get(m, id)?.map(|r| r.object(m)).transpose()?,
            _ => None,
        };
        actors.push(json!({
            "datum": format!("{:08x}", handle.raw()),
            "unit": (unit != u32::MAX).then(|| ObjectId { kind: Kind::Actor, key: unit }.to_string()),
            "character": (character != u32::MAX).then(|| format!("{character:08x}")),
            "team": a.team(m)?,
            "focus_object": focus.filter(|&v| v != u32::MAX).map(|v| format!("{v:08x}")),
            "squad": (squad >= 0).then_some(squad),
            "behavior": behavior,
            "behavior_name": behavior_name,
            "combat_status": a.combat_status(m)?,
            "location_cached": cached,
            "sector": (cached && (0..count).contains(&sector)).then_some(sector),
        }));
    }
    Ok(actors)
}

/// Registers the read-only actor snapshot.
pub fn install(agent: &Agent) {
    agent.add_game_method("world.ai", Access::Read, |p, ctx| {
        if !p.is_null() && !p.as_object().is_some_and(|o| o.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "world.ai takes no parameters",
            ));
        }
        let actors = ctx.with_guest(|m| {
            snapshot(m).map_err(|e| {
                RpcError::new(code::UNAVAILABLE, format!("reading AI at {:#x}", e.addr))
            })
        })?;
        Ok(json!({ "at": ctx.at, "actors": actors }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::{testing::FakeGuest, view::fake};

    #[test]
    fn reports_the_active_frame_and_only_a_current_in_range_sector() {
        let mut m = FakeGuest::new();
        m.put_u32(ACTORS, 0);
        assert!(snapshot(&m).unwrap().is_empty());
        let pool = fake::datums::<Actor>(&mut m, 0x10000, 4);
        let (id, a) = fake::push(&mut m, &pool);
        let props = fake::datums::<PropRef>(&mut m, 0x30000, 2);
        let (focus, target) = fake::push(&mut m, &props);
        m.put_u32(PROP_REFS, props.array.addr)
            .put_u32(a.addr + 0x338, focus.raw())
            .put_u32(target.addr + 0x20, 0xe1230042);
        m.put_u32(ACTORS, pool.array.addr)
            .put_u32(ACTIVE_BSP, 0x20000)
            .put_u32(0x200c4, 1)
            .put_u32(0x200c8, 0x21000)
            .put_u32(0x21000, 8);
        m.put(0x40000, b"invented behavior\0");
        m.put_u32(BEHAVIOR_NAMES + 4 * 17, 0x40000);
        m.put_u32(a.addr + 0x18, 0xabcd0002)
            .put_u32(a.addr + 0x30, 5)
            .put_u16(a.addr + 0x86, 6)
            .put_u16(a.addr + 0x190, 2)
            .put_u16(a.addr + 0x110, 17)
            .put_u8(a.addr + 0x278, 1)
            .put_u32(a.addr + 0x28c, 7);
        let row = &snapshot(&m).unwrap()[0];
        assert_eq!(row["datum"], format!("{:08x}", id.raw()));
        assert_eq!(row["unit"], "actor.abcd0002");
        assert_eq!(row["behavior"], 17);
        assert_eq!(row["behavior_name"], "invented behavior");
        assert_eq!(row["focus_object"], "e1230042");
        assert_eq!(row["combat_status"], 6);
        assert_eq!(row["squad"], 5);
        assert_eq!(row["sector"], 7);
        m.put_u8(a.addr + 0x278, 0);
        assert!(snapshot(&m).unwrap()[0]["sector"].is_null());
        m.put_u8(a.addr + 0x278, 1)
            .put_u32(a.addr + 0x28c, 8)
            .put_u16(a.addr + 0x190, 4)
            .put_u32(a.addr + 0x338, focus.raw() ^ (1 << 16));
        let row = &snapshot(&m).unwrap()[0];
        assert!(row["sector"].is_null());
        assert!(row["behavior"].is_null());
        assert!(row["behavior_name"].is_null());
        assert!(row["focus_object"].is_null());
    }
}
