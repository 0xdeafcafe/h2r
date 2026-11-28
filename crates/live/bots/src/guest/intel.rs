//! Extra, optional reads for the multiplayer decision layer.

use super::{DatumHandle, GuestMemory, R, objects, points};
use crate::decisions::supplies::{Powerup, Supply};
use guest_layout::tags::GroupTag;
use guest_layout::view::Tags;

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
    if meta == 0 || tag.group(m)? != GroupTag::from_chars(*b"eqip") {
        return Ok(None);
    }
    let kind = match m.u16(meta + 0x12c)? {
        2 => Powerup::Overshield,
        3 => Powerup::Camo,
        _ => return Ok(None),
    };
    supply(m, object, body, Some(kind), crate::arms::Role::Unknown)
}

/// Correlate an object's signed placement index with the nearby scenario pad.
/// The old instance can outlive the spawner's next object.
pub(super) struct Placement {
    pub site: u8,
    rec: u32,
    spawn: crate::brain::Vec3,
    pub(super) at: crate::brain::Vec3,
}

impl Placement {
    pub(super) fn read(m: &dyn GuestMemory, body: objects::Body) -> R<Option<Self>> {
        let site = m.u8(body.addr + 0xaf)?;
        if site >= 100 {
            return Ok(None);
        }
        let scenario = m.u32(points::SCENARIO.base)?;
        if scenario == 0 || i32::from(site) >= m.i32(scenario + 0x120)? {
            return Ok(None);
        }
        let base = m.u32(scenario + 0x124)?;
        if base == 0 {
            return Ok(None);
        }
        let rec = base + u32::from(site) * 0x90;
        let spawn = m.vec3(rec + 0x40)?;
        let at = body.position(m)?;
        let d = crate::decisions::distance(spawn, at);
        Ok((d.is_finite() && d <= 1.5).then_some(Self {
            site,
            rec,
            spawn,
            at,
        }))
    }

    pub(super) fn registered(&self, m: &dyn GuestMemory) -> R<Option<u32>> {
        let engine = m.u32(points::GAME_ENGINE.base)?;
        if engine == 0 {
            return Ok(None);
        }
        Ok(Some(m.u32(engine + 0x7dc + 8 * u32::from(self.site))?))
    }
}

pub(crate) fn supply(
    m: &dyn GuestMemory,
    object: DatumHandle,
    body: objects::Body,
    powerup: Option<Powerup>,
    role: crate::arms::Role,
) -> R<Option<Supply>> {
    let Some(p) = Placement::read(m, body)? else {
        return Ok(None);
    };
    if p.registered(m)
        .ok()
        .flatten()
        .is_some_and(|h| h != object.raw())
    {
        return Ok(None);
    }
    let Placement {
        site,
        rec,
        at,
        spawn,
    } = p;
    let mut seconds = m.u16(rec + 0x0e)? as i16;
    if seconds == 0
        && [u32::from_be_bytes(*b"itmc"), u32::from_be_bytes(*b"vehc")]
            .contains(&m.u32(rec + 0x58)?)
        && let Some(def) = DatumHandle::from_raw(m.u32(rec + 0x5c)?)
        && let Some(tag) = Tags.entry(m, def)?
        && [
            GroupTag::from_chars(*b"itmc"),
            GroupTag::from_chars(*b"vehc"),
        ]
        .contains(&tag.group(m)?)
    {
        let meta = tag.address(m)?;
        if meta != 0 {
            seconds = m.u16(meta + 8)? as i16;
        }
    }
    let mut period = if seconds == 0 { 30 } else { i32::from(seconds) };
    if let Ok(globals) = m.u32(points::GAME_GLOBALS.base)
        && globals != 0
        && let Ok(class) = m.u8(rec + 0x14)
    {
        let option = match class {
            0 => Some(0x211),
            1..=7 => Some(0x208),
            _ => None,
        };
        match option.and_then(|offset| m.u8(globals + offset).ok()) {
            Some(1) => period = -1,
            Some(2) => period /= 2,
            Some(3) => period *= 2,
            _ => {}
        }
    }
    let rate = service_rate(m).unwrap_or(30);
    Ok(Some(Supply {
        site,
        object: object.raw(),
        spawn,
        at,
        respawn: (period > 0).then(|| (period as u32).saturating_mul(u32::from(rate))),
        disabled: period < 0,
        role,
        on_disturbance: m.u16(rec + 0x12)? == 0,
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

pub(super) fn service_rate(m: &dyn GuestMemory) -> Option<u16> {
    m.u32(points::GAME_TIME.base)
        .ok()
        .filter(|&time| time != 0)
        .and_then(|time| m.u16(time + 2).ok())
        .filter(|&r| r != 0)
}

/// One placement is serviced per second at tick % rate == index % rate.
/// The decrement that reaches zero calls the spawner in that same update.
#[cfg(test)]
pub(super) fn clocks(m: &dyn GuestMemory, tick: u32) -> R<Vec<crate::decisions::supplies::Clock>> {
    use crate::decisions::supplies::Clock;
    let e = m.u32(points::GAME_ENGINE.base)?;
    let s = m.u32(points::SCENARIO.base)?;
    let time = m.u32(points::GAME_TIME.base)?;
    if e == 0 || s == 0 || time == 0 {
        return Ok(vec![]);
    }
    let rate = u32::from(m.u16(time + 2)?);
    if rate == 0 {
        return Ok(vec![]);
    }
    let count = m.i32(s + 0x120)?.clamp(0, 100) as usize;
    let mut records = [0; 800];
    let records = &mut records[..count * 8];
    m.read(e + 0x7dc, records)?;
    Ok(records
        .chunks_exact(8)
        .enumerate()
        .map(|(i, rec)| {
            let remaining = i16::from_le_bytes([rec[4], rec[5]]);
            let phase = (i as u32 % rate + rate - tick % rate) % rate;
            let service = tick.wrapping_add(if phase == 0 { rate } else { phase });
            match remaining {
                i16::MIN..=-1 => Clock::Disabled,
                0 => Clock::Idle {
                    service,
                    rate,
                    deleted: rec[..4] == [0xff; 4],
                },
                _ => Clock::Due(service.wrapping_add((remaining as u32 - 1) * rate)),
            }
        })
        .collect())
}
