//! Extra, optional reads for the multiplayer decision layer.

use super::{DatumHandle, GuestMemory, R, objects, points};
use crate::decisions::supplies::Supply;
use engine::tags::GroupTag;
use engine::view::Tags;

pub(crate) const EQUIPMENT: u8 = 3;

pub(crate) fn equipment(m: &dyn GuestMemory, object: DatumHandle, addr: u32) -> R<Option<Supply>> {
    if addr == 0 {
        return Ok(None);
    }
    let body = objects::Body::at(addr);
    if body.parent(m)?.is_some() {
        return Ok(None);
    }
    let Some(def) = body.definition(m)? else {
        return Ok(None);
    };
    let Some(tag) = Tags.entry(m, def)? else {
        return Ok(None);
    };
    let meta = tag.address(m)?;
    if meta == 0
        || tag.group(m)? != GroupTag::from_chars(*b"eqip")
        || !matches!(m.u16(meta + 0x12c)?, 2 | 3)
    {
        return Ok(None);
    }
    supply(m, object, body, true)
}

// The same placement index and layout as live-telemetry::layout and
// scenario-edit's NetgameEquipment. A weapon dropped away from its pad
// must never start a second timer.
pub(crate) fn supply(
    m: &dyn GuestMemory,
    object: DatumHandle,
    body: objects::Body,
    powerup: bool,
) -> R<Option<Supply>> {
    let index = m.u8(body.addr + 0xaf)? as i8;
    if index < 0 {
        return Ok(None);
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    if scenario == 0 || i32::from(index) >= m.i32(scenario + 0x120)? {
        return Ok(None);
    }
    let base = m.u32(scenario + 0x124)?;
    if base == 0 {
        return Ok(None);
    }
    let rec = base + index as u32 * 0x90;
    let spawn = m.vec3(rec + 0x40)?;
    let at = body.position(m)?;
    let d = crate::decisions::distance(spawn, at);
    if !d.is_finite() || d > 1.5 {
        return Ok(None);
    }
    let mut seconds = m.u16(rec + 0x0e)? as i16;
    if seconds == 0
        && let Some(def) = DatumHandle::from_raw(m.u32(rec + 0x5c)?)
        && let Some(tag) = Tags.entry(m, def)?
        && tag.group(m)? == GroupTag::from_chars(*b"itmc")
    {
        let meta = tag.address(m)?;
        if meta != 0 {
            seconds = m.u16(meta + 8)? as i16;
        }
    }
    Ok(Some(Supply {
        site: index as u8,
        object: object.raw(),
        spawn,
        at,
        respawn: (seconds > 0).then(|| seconds as u32 * 30),
        powerup,
    }))
}

pub(super) fn held_model(m: &dyn GuestMemory, unit: u32) -> R<Option<u32>> {
    let slot = u32::from(m.u8(unit + crate::arms::UNIT_WEAPON_INDEX)?);
    if slot >= crate::arms::UNIT_WEAPON_SLOTS {
        return Ok(None);
    }
    let Some(id) = DatumHandle::from_raw(m.u32(unit + crate::arms::UNIT_WEAPONS + 4 * slot)?)
    else {
        return Ok(None);
    };
    let Some((objects::WEAPON, body)) = objects::get(m, id)? else {
        return Ok(None);
    };
    Ok(body.definition(m)?.map(|d| d.raw()))
}

/// Scoreboard rows, shared layout with live-telemetry::memory::team_scores.
pub(super) fn ahead(m: &dyn GuestMemory, me: u32, teams: bool, roster: &[(u32, i16)]) -> R<bool> {
    let engine = m.u32(crate::objective::GAME_ENGINE)?;
    if engine == 0 || m.u32(engine + 0x304)? == 0 {
        return Ok(false);
    }
    let row = |id, team| {
        if teams {
            (0..8)
                .contains(&team)
                .then(|| engine + 0x4c8 + u32::from(team as u16) * 0x12)
        } else {
            (id < 16).then(|| engine + 0x308 + id * 0x1c)
        }
    };
    let Some(&(_, team)) = roster.iter().find(|&&(id, _)| id == me) else {
        return Ok(false);
    };
    let Some(ours) = row(me, team) else {
        return Ok(false);
    };
    let score = m.u16(ours)? as i16;
    let mut contested = false;
    for &(id, t) in roster {
        if let Some(theirs) = row(id, t)
            && theirs != ours
        {
            if m.u16(theirs)? as i16 >= score {
                return Ok(false);
            }
            contested = true;
        }
    }
    Ok(contested)
}
