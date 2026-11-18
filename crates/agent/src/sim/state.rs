//! `sim.state.set`'s host writes and the `player` fields `sim.view` adds,
//! at the addresses Live telemetry reads (`live_telemetry::layout`,
//! `engine::view`): the player record's team, and its unit's position,
//! velocity, team, vitality, grenades and held weapon's magazine.
//!
//! Every value is checked before the first write, so a refused set
//! writes nothing.

use agent_proto::sim::SetParams;
use agent_proto::{RpcError, code};
use engine::GuestMemory;
use engine::view::objects::{self, Body};
use engine::view::players::{self, Player};
use live_telemetry::layout as l;
use serde_json::{Value, json};

/// The highest shield vitality accepted: an overshield charges to 3.
pub const SHIELD_MAX: f64 = 3.0;
/// The players array holds 16.
pub const PLAYERS: u32 = 16;

fn fault(e: engine::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("guest memory at {:#x} faulted", e.addr),
    )
}

/// The listed player in slot `index`.
pub fn player(m: &dyn GuestMemory, index: u32) -> Result<Player, RpcError> {
    let gone = || RpcError::new(code::GONE, format!("no player.{index}"));
    if index >= PLAYERS {
        return Err(gone());
    }
    let array = players::array(m)
        .map_err(fault)?
        .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no players array (no game loaded)"))?;
    let p = array
        .get_absolute(m, index as i32)
        .map_err(fault)?
        .ok_or_else(gone)?;
    if !p.listed(m).map_err(fault)? {
        return Err(gone());
    }
    Ok(p)
}

/// The player's unit's body, `None` between lives.
pub fn unit(m: &dyn GuestMemory, p: Player) -> Result<Option<Body>, RpcError> {
    let Some(h) = p.unit(m).map_err(fault)? else {
        return Ok(None);
    };
    Ok(objects::get(m, h).map_err(fault)?.map(|(_, b)| b))
}

/// The held weapon's body, if the unit holds one.
fn held_weapon(m: &dyn GuestMemory, unit: Body) -> Result<Option<Body>, RpcError> {
    let i = m.u8(unit.addr + l::UNIT_WEAPON_INDEX).map_err(fault)? as i8;
    if !(0..l::UNIT_WEAPON_SLOTS).contains(&i) {
        return Ok(None);
    }
    let datum = m
        .u32(unit.addr + l::UNIT_WEAPONS + 4 * i as u32)
        .map_err(fault)?;
    let Some(h) = engine::DatumHandle::from_raw(datum) else {
        return Ok(None);
    };
    Ok(objects::get(m, h).map_err(fault)?.map(|(_, b)| b))
}

/// What `sim.view` adds to a player: `health`, `shield`, `grenades`
/// (`frag`, `plasma`) and the held weapon's `ammo` (`loaded`, `reserve`).
/// Empty between lives.
pub fn vitals(m: &dyn GuestMemory, index: u32) -> Result<serde_json::Map<String, Value>, RpcError> {
    let mut out = serde_json::Map::new();
    let p = player(m, index)?;
    let Some(u) = unit(m, p)? else {
        return Ok(out);
    };
    out.insert("health".into(), json!(u.health(m).map_err(fault)?));
    out.insert("shield".into(), json!(u.shield(m).map_err(fault)?));
    let g = |i: u32| m.u8(u.addr + l::UNIT_GRENADE_COUNTS + i).map(|v| v as i8);
    out.insert(
        "grenades".into(),
        json!({ "frag": g(0).map_err(fault)?, "plasma": g(1).map_err(fault)? }),
    );
    if let Some(w) = held_weapon(m, u)? {
        let a = |o: u32| m.u16(w.addr + o).map(|v| v as i16);
        out.insert(
            "ammo".into(),
            json!({
                "loaded": a(l::WEAPON_MAGAZINE_LOADED).map_err(fault)?,
                "reserve": a(l::WEAPON_MAGAZINE_RESERVE).map_err(fault)?,
            }),
        );
    }
    Ok(out)
}

fn bad(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

/// Checks the values alone (no guest).
pub fn validate(p: &SetParams) -> Result<(), RpcError> {
    if let Some(pos) = p.pos
        && !pos.iter().all(|v| v.is_finite() && v.abs() < 1e6)
    {
        return Err(bad("pos: three finite world coordinates"));
    }
    if let Some(t) = p.team
        && !(0..16).contains(&t)
    {
        return Err(bad("team: 0 to 15"));
    }
    if let Some(h) = p.health
        && !(h.is_finite() && (0.0..=1.0).contains(&h))
    {
        return Err(bad("health: 0 to 1"));
    }
    if let Some(s) = p.shield
        && !(s.is_finite() && (0.0..=SHIELD_MAX).contains(&s))
    {
        return Err(bad(format!("shield: 0 to {SHIELD_MAX}")));
    }
    if let Some(g) = p.grenades
        && g.frag.iter().chain(&g.plasma).any(|n| *n < 0)
    {
        return Err(bad("grenades: 0 or more"));
    }
    if let Some(a) = p.ammo
        && a.loaded.iter().chain(&a.reserve).any(|n| *n < 0)
    {
        return Err(bad("ammo: 0 or more"));
    }
    let any = p.pos.is_some()
        || p.team.is_some()
        || p.health.is_some()
        || p.shield.is_some()
        || p.grenades.is_some()
        || p.ammo.is_some();
    if !any {
        return Err(bad(
            "nothing to set: pos, team, health, shield, grenades or ammo",
        ));
    }
    Ok(())
}

/// Writes `p` to player `index`; the fields written. Checks everything
/// (values, the unit, the held weapon) before the first write.
pub fn apply(
    m: &mut dyn GuestMemory,
    index: u32,
    p: &SetParams,
) -> Result<Vec<&'static str>, RpcError> {
    validate(p)?;
    let pl = player(m, index)?;
    let needs_unit = p.pos.is_some()
        || p.health.is_some()
        || p.shield.is_some()
        || p.grenades.is_some()
        || p.ammo.is_some();
    let u = unit(m, pl)?;
    if needs_unit && u.is_none() {
        return Err(RpcError::new(
            code::GONE,
            format!("player.{index} has no unit now (between lives)"),
        ));
    }
    let weapon = match (p.ammo, u) {
        (Some(_), Some(u)) => Some(held_weapon(m, u)?.ok_or_else(|| {
            RpcError::new(code::GONE, format!("player.{index} holds no weapon"))
        })?),
        _ => None,
    };
    let mut wrote = Vec::new();
    if let Some(t) = p.team {
        pl.set_team(m, t as i8).map_err(fault)?;
        if let Some(u) = u {
            m.set_u16(u.addr + l::UNIT_TEAM, t as u16).map_err(fault)?;
        }
        wrote.push("team");
    }
    let Some(u) = u else {
        return Ok(wrote);
    };
    if let Some(pos) = p.pos {
        u.set_position(m, pos.map(|v| v as f32)).map_err(fault)?;
        for i in 0..3 {
            m.set_f32(u.addr + l::BODY_VELOCITY + 4 * i, 0.0)
                .map_err(fault)?;
        }
        wrote.push("pos");
    }
    if let Some(h) = p.health {
        u.set_health(m, h as f32).map_err(fault)?;
        wrote.push("health");
    }
    if let Some(s) = p.shield {
        u.set_shield(m, s as f32).map_err(fault)?;
        wrote.push("shield");
    }
    if let Some(g) = p.grenades {
        for (i, n) in [(0, g.frag), (1, g.plasma)] {
            if let Some(n) = n {
                m.write(u.addr + l::UNIT_GRENADE_COUNTS + i, &[n as u8])
                    .map_err(fault)?;
            }
        }
        wrote.push("grenades");
    }
    if let (Some(a), Some(w)) = (p.ammo, weapon) {
        for (o, n) in [
            (l::WEAPON_MAGAZINE_LOADED, a.loaded),
            (l::WEAPON_MAGAZINE_RESERVE, a.reserve),
        ] {
            if let Some(n) = n {
                m.set_u16(w.addr + o, n as u16).map_err(fault)?;
            }
        }
        wrote.push("ammo");
    }
    Ok(wrote)
}
