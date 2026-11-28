//! Pickup time belongs to an event. The placement table holds a countdown.
//! Both observers run before the engine detaches the object from its pad
//! or destroys it. See docs/game/bots.md §3.6.
use super::{DatumHandle, GuestMemory, R, game_tick, objects, players};
use crate::decisions::supplies::Take;
use std::sync::Mutex;

static LAST: Mutex<[Option<Take>; 100]> = Mutex::new([None; 100]);

pub fn clear() {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).fill(None);
}

pub(super) fn recent_into(tick: u32, out: &mut Vec<Take>) {
    out.clear();
    out.extend(
        LAST.lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .flatten()
            .copied()
            .filter(|e| tick.wrapping_sub(e.tick) <= 30),
    );
}

/// Item attachment at 0x10ca80 also handles detachment (no unit).
pub fn weapon(m: &dyn GuestMemory, object: u32, unit: u32) {
    if let Ok(Some(event)) = take(m, object, unit, objects::WEAPON, None) {
        store(event);
    }
}

/// 0xa89c0's only caller is the success path of 0x151ea0. Its caller's
/// second argument is the equipment datum, still live before deletion.
pub fn equipment(m: &dyn GuestMemory, sp: u32, unit: u32, definition: u32) {
    if let Ok(Some(event)) = equipment_event(m, sp, unit, definition) {
        store(event);
    }
}

pub(super) fn equipment_event(
    m: &dyn GuestMemory,
    sp: u32,
    unit: u32,
    definition: u32,
) -> R<Option<Take>> {
    let Some(object) = consumed_equipment(m, sp)? else {
        return Ok(None);
    };
    take(
        m,
        object.raw(),
        unit,
        super::intel::EQUIPMENT,
        Some(definition),
    )
}

/// The equipment datum delivered to the verified successful pickup call site.
pub fn consumed_equipment(
    m: &dyn GuestMemory,
    sp: u32,
) -> Result<Option<DatumHandle>, guest_layout::MemFault> {
    if m.u32(sp)? != 0x151fd2 {
        return Ok(None);
    }
    Ok(DatumHandle::from_raw(m.u32(sp.wrapping_add(0x20))?))
}

fn store(event: Take) {
    LAST.lock().unwrap_or_else(|e| e.into_inner())[usize::from(event.site)] = Some(event);
}

pub(super) fn take(
    m: &dyn GuestMemory,
    object: u32,
    unit: u32,
    kind: u8,
    definition: Option<u32>,
) -> R<Option<Take>> {
    let (Some(object), Some(unit), Some(tick)) = (
        DatumHandle::from_raw(object),
        DatumHandle::from_raw(unit),
        game_tick(m),
    ) else {
        return Ok(None);
    };
    let Some((ty, item)) = objects::get(m, object)? else {
        return Ok(None);
    };
    if ty != kind
        || item.parent(m)?.is_some()
        || definition.is_some() && item.definition(m)?.map(|h| h.raw()) != definition
    {
        return Ok(None);
    }
    let Some(placement) = super::intel::Placement::read(m, item)? else {
        return Ok(None);
    };
    if placement.registered(m)? != Some(object.raw()) {
        return Ok(None);
    }
    let site = placement.site;
    let Some((objects::BIPED, biped)) = objects::get(m, unit)? else {
        return Ok(None);
    };
    let Some(player) = DatumHandle::from_raw(m.u32(biped.addr + 0x13c)?) else {
        return Ok(None);
    };
    let Some(array) = players::array(m)? else {
        return Ok(None);
    };
    let Some(p) = array.get(m, player)? else {
        return Ok(None);
    };
    if !p.listed(m)? || p.unit(m)? != Some(unit) {
        return Ok(None);
    }
    Ok(Some(Take {
        at: placement.at,
        site,
        object: object.raw(),
        unit: unit.raw(),
        player: u32::from(player.index()),
        tick,
    }))
}
