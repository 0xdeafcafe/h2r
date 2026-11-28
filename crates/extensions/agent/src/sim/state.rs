//! `sim.state.set`'s host writes and the `player` fields `sim.view` adds,
//! through the typed guest views (`guest_layout`, `guest_layout::view`): the
//! player record's team, and its unit's team, vitality, grenades and held
//! weapon's magazine.
//!
//! A position or facing is not a write here: the biped's physics put the
//! body back at the next tick. They go to the game's own teleport
//! ([`super::calls`]).
//!
//! Every value is checked before the first write, so a refused set
//! writes nothing.

use agent_proto::sim::SetParams;
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::view::objects::{self, Body};
use guest_layout::view::players::{self, Player};
use guest_layout::{Off, Unit, Weapon};
use serde_json::{Value, json};

/// The highest shield vitality accepted: an overshield charges to 3.
pub const SHIELD_MAX: f64 = 3.0;
/// The players array holds 16.
pub const PLAYERS: u32 = 16;

fn fault(e: guest_layout::MemFault) -> RpcError {
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

/// The player's salted handle (what the game's player functions take).
pub fn player_handle(m: &dyn GuestMemory, index: u32) -> Result<u32, RpcError> {
    let p = player(m, index)?;
    let salt = p.salt(m).map_err(fault)?;
    Ok(guest_layout::DatumHandle::new(salt, index).raw())
}

/// The player's unit's handle; `GONE` between lives.
pub fn unit_handle(m: &dyn GuestMemory, index: u32) -> Result<u32, RpcError> {
    let p = player(m, index)?;
    match p.unit(m).map_err(fault)? {
        Some(h) if objects::get(m, h).map_err(fault)?.is_some() => Ok(h.raw()),
        _ => Err(RpcError::new(
            code::GONE,
            format!("player.{index} has no unit now (between lives)"),
        )),
    }
}

/// The weapon object the unit holds now, as 8 hex digits.
pub fn held_object(m: &dyn GuestMemory, index: u32) -> Option<String> {
    let u = unit(m, player(m, index).ok()?).ok()??;
    let d = Unit::at(u.addr).held_weapon(m).ok()??;
    Some(format!("{:08x}", d.raw()))
}

/// Writes a weapon object's first magazine (the fields `ammo` names).
pub fn set_magazine(
    m: &mut dyn GuestMemory,
    weapon: u32,
    a: agent_proto::sim::Ammo,
) -> Result<(), RpcError> {
    let h = guest_layout::DatumHandle::from_raw(weapon)
        .ok_or_else(|| RpcError::new(code::GONE, "no weapon"))?;
    let (_, w) = objects::get(m, h)
        .map_err(fault)?
        .ok_or_else(|| RpcError::new(code::GONE, format!("weapon {weapon:08x} is gone")))?;
    write_magazine(m, Weapon::at(w.addr), a)
}

/// The first magazine's loaded and reserve rounds, each when given.
fn write_magazine(
    m: &mut dyn GuestMemory,
    w: Weapon,
    a: agent_proto::sim::Ammo,
) -> Result<(), RpcError> {
    for (field, n) in [
        (Weapon::MAGAZINE_LOADED, a.loaded),
        (Weapon::MAGAZINE_RESERVE, a.reserve),
    ] {
        if let Some(n) = n {
            w.set(m, field, n).map_err(fault)?;
        }
    }
    Ok(())
}

/// Checks an `ammo` value alone.
pub fn validate_ammo(a: &agent_proto::sim::Ammo) -> Result<(), RpcError> {
    if a.loaded.iter().chain(&a.reserve).any(|n| *n < 0) {
        return Err(bad("ammo: 0 or more"));
    }
    Ok(())
}

/// The held weapon's body, if the unit holds one.
fn held_weapon(m: &dyn GuestMemory, unit: Body) -> Result<Option<Body>, RpcError> {
    let Some(datum) = Unit::at(unit.addr).held_weapon(m).map_err(fault)? else {
        return Ok(None);
    };
    let Some(h) = guest_layout::DatumHandle::from_raw(datum.raw()) else {
        return Ok(None);
    };
    Ok(objects::get(m, h).map_err(fault)?.map(|(_, b)| b))
}

/// Player: lives remaining, `i16`; the engine writes it, -1 unlimited
/// (docs/game/firefight.md, "Guest boundary", lives and respawn).
pub const PLAYER_LIVES: u32 = 0x1ac;
/// The game engine globals' per-player stat rows: `0x1c` bytes from `+0x308`,
/// the score an `i16` at the row's `+0x00` (`SCORE_ADD`, `0x15b7c0`).
pub const STAT_ROWS: u32 = 0x308;
pub const STAT_ROW_SIZE: u32 = 0x1c;

/// The player's lives remaining, `None` when unlimited (-1).
pub fn lives(m: &dyn GuestMemory, p: Player) -> Result<Option<i16>, RpcError> {
    let lives = m.u16(p.addr.wrapping_add(PLAYER_LIVES)).map_err(fault)? as i16;
    Ok((lives != -1).then_some(lives))
}

/// Player `index`'s score in the game engine's stat row; `None` without a
/// game engine (the menu, a campaign) or when its globals do not read.
pub fn score(m: &dyn GuestMemory, index: u32) -> Option<i16> {
    let engine = m.u32(guest_layout::points::GAME_ENGINE.base).ok().filter(|&e| e != 0)?;
    let row = engine.wrapping_add(STAT_ROWS + STAT_ROW_SIZE * index);
    m.u16(row).ok().map(|s| s as i16)
}

/// What `sim.view` adds to a player: `lives` (null when unlimited) and
/// `score` (null without a game engine) always, and with a unit `health`,
/// `shield`, `grenades` (`frag`, `plasma`) and the held weapon's `ammo`
/// (`loaded`, `reserve`).
pub fn vitals(m: &dyn GuestMemory, index: u32) -> Result<serde_json::Map<String, Value>, RpcError> {
    let mut out = serde_json::Map::new();
    let p = player(m, index)?;
    out.insert("lives".into(), json!(lives(m, p)?));
    out.insert("score".into(), json!(score(m, index)));
    let Some(u) = unit(m, p)? else {
        return Ok(out);
    };
    out.insert("health".into(), json!(u.health(m).map_err(fault)?));
    out.insert("shield".into(), json!(u.shield(m).map_err(fault)?));
    let [frag, plasma] = Unit::at(u.addr).grenade_counts(m).map_err(fault)?;
    out.insert("grenades".into(), json!({ "frag": frag, "plasma": plasma }));
    if let Some(w) = held_weapon(m, u)? {
        let w = Weapon::at(w.addr);
        out.insert(
            "ammo".into(),
            json!({
                "loaded": w.magazine_loaded(m).map_err(fault)?,
                "reserve": w.magazine_reserve(m).map_err(fault)?,
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
    if let Some(a) = &p.ammo {
        validate_ammo(a)?;
    }
    p.facing_vector().map_err(bad)?;
    let any = p.pos.is_some()
        || p.yaw.is_some()
        || p.facing.is_some()
        || p.team.is_some()
        || p.health.is_some()
        || p.shield.is_some()
        || p.grenades.is_some()
        || p.ammo.is_some();
    if !any {
        return Err(bad(
            "nothing to set: pos, yaw/pitch or facing, team, health, shield, grenades or ammo",
        ));
    }
    Ok(())
}

/// Writes `p` to player `index`; the fields written. Checks everything
/// (values, the unit, the held weapon) before the first write. `pos` and
/// the facing are not written here (the module doc): only checked to have
/// a unit to move.
pub fn apply(
    m: &mut dyn GuestMemory,
    index: u32,
    p: &SetParams,
) -> Result<Vec<&'static str>, RpcError> {
    validate(p)?;
    let pl = player(m, index)?;
    let needs_unit = p.pos.is_some()
        || p.yaw.is_some()
        || p.facing.is_some()
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
    let weapon =
        match (p.ammo, u) {
            (Some(_), Some(u)) => Some(held_weapon(m, u)?.ok_or_else(|| {
                RpcError::new(code::GONE, format!("player.{index} holds no weapon"))
            })?),
            _ => None,
        };
    let mut wrote = Vec::new();
    if let Some(t) = p.team {
        pl.set_team(m, t as i8).map_err(fault)?;
        if let Some(u) = u {
            Unit::at(u.addr).set(m, Unit::TEAM, t as i16).map_err(fault)?;
        }
        wrote.push("team");
    }
    let Some(u) = u else {
        return Ok(wrote);
    };
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
                // One count of the pair, so the other is left as it is.
                let count = Off::<i8>::new(Unit::GRENADE_COUNTS.offset + i);
                Unit::at(u.addr).set(m, count, n).map_err(fault)?;
            }
        }
        wrote.push("grenades");
    }
    if let (Some(a), Some(w)) = (p.ammo, weapon) {
        write_magazine(m, Weapon::at(w.addr), a)?;
        wrote.push("ammo");
    }
    Ok(wrote)
}
