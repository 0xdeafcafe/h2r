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
