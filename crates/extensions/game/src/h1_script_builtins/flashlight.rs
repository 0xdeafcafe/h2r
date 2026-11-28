//! The staged flashlight requests drive the unit's integrated-light
//! power. The active weapon reads that same value for its attachments.

use std::collections::BTreeSet;

use guest_layout::{
    datum::DatumArray,
    handle::DatumHandle,
    memory::{GuestMemory, MemFault},
    points,
    view::objects::{self, Body},
};
use hooks::{Builder, Flow};

const LISTS: u32 = 0x004f_55d8;
const LIST_NODES: u32 = 0x004f_55d4;
const UNIT_FLAGS: u32 = 0x134;
const LIGHT_ON: u32 = 1 << 29;
const LIGHT_POWER: u32 = 0x264;
const LIGHT_SECONDS: u32 = 0x268;
const WEAPON_SLOT: u32 = 0x212;
const WEAPONS: u32 = 0x218;
const INTEGRATED_LIGHT_POWER: u32 = 0x1600_05a0;

fn unit(m: &dyn GuestMemory, handle: u32) -> Result<Option<Body>, MemFault> {
    Ok(objects::get(m, DatumHandle(handle))?
        .filter(|(kind, _)| matches!(*kind, objects::BIPED | objects::VEHICLE))
        .map(|(_, body)| body))
}

pub(super) fn is_on(m: &dyn GuestMemory, handle: u32) -> Result<bool, MemFault> {
    Ok(match unit(m, handle)? {
        Some(body) => m.u32(body.addr + UNIT_FLAGS)? & LIGHT_ON != 0,
        None => false,
    })
}

pub(super) fn set(m: &mut dyn GuestMemory, handle: u32, on: bool) -> Result<(), MemFault> {
    let Some(body) = unit(m, handle)? else {
        return Ok(());
    };
    let flags = m.u32(body.addr + UNIT_FLAGS)?;
    let new_flags = if on { flags | LIGHT_ON } else { flags & !LIGHT_ON };
    if flags != new_flags {
        m.set_u32(body.addr + UNIT_FLAGS, new_flags)?;
        if on && m.u32(body.addr + crate::health::UNIT_PLAYER)? != u32::MAX {
            // The native player branch uses this countdown to hold the
            // flashlight on. Halo 1 drains a full battery over 120 s.
            m.set_f32(body.addr + LIGHT_SECONDS, 120.0)?;
        }
    }
    Ok(())
}

pub(super) fn set_list(m: &mut dyn GuestMemory, list: u32, on: bool) -> Result<(), MemFault> {
    if list == u32::MAX {
        return Ok(());
    }
    let lists = DatumArray::at(m.u32(LISTS)?);
    if let Some(header) = lists.try_get(m, DatumHandle(list))? {
        let nodes = DatumArray::at(m.u32(LIST_NODES)?);
        let mut next = m.u32(header + 8)?;
        let mut seen = BTreeSet::new();
        while seen.insert(next) && let Some(node) = nodes.try_get(m, DatumHandle(next))? {
            let unit = m.u32(node + 4)?;
            set(m, unit, on)?;
            next = m.u32(node + 8)?;
        }
    }
    Ok(())
}

fn interpolate(power: f32, on: bool) -> f32 {
    // Halo 1's unit light at 0x45bc0 / 0x45c1f, six ticks up and 24 down.
    let delta = if on { 1.0 / 6.0 } else { -1.0 / 24.0 };
    (power + delta).clamp(0.0, 1.0)
}

fn npc_light(m: &dyn GuestMemory, handle: u32) -> Result<Option<(u32, u32, f32)>, MemFault> {
    let Some(body) = unit(m, handle)? else { return Ok(None) };
    if m.u32(body.addr + crate::health::UNIT_PLAYER)? != u32::MAX {
        return Ok(None);
    }
    let flags = m.u32(body.addr + UNIT_FLAGS)?;
    let power = m.f32(body.addr + LIGHT_POWER)?;
    if flags & LIGHT_ON == 0 && power <= 0.0 {
        return Ok(None);
    }
    let on = flags & LIGHT_ON != 0 && body.damage_flags(m)? & 4 == 0 && body.parent(m)?.is_none();
    let flags = if on { flags } else { flags & !LIGHT_ON };
    Ok(Some((body.addr, flags, interpolate(power, on))))
}

/// Halo 1 writes the unit's power to the active weapon at 0x45741.
/// Halo 2 can resolve it directly at the native function import boundary.
fn weapon_power(m: &dyn GuestMemory, handle: u32) -> Result<Option<f32>, MemFault> {
    let Some((objects::WEAPON, weapon)) = objects::get(m, DatumHandle(handle))? else {
        return Ok(None);
    };
    let Some(holder) = weapon.parent(m)? else {
        return Ok(Some(0.0));
    };
    let Some(holder) = unit(m, holder.raw())? else {
        return Ok(Some(0.0));
    };
    let slot = u32::from(m.u8(holder.addr + WEAPON_SLOT)?);
    if slot >= 4 || m.u32(holder.addr + WEAPONS + 4 * slot)? != handle {
        return Ok(Some(0.0));
    }
    Ok(Some(m.f32(holder.addr + LIGHT_POWER)?))
}

pub(super) fn install(b: &mut Builder) {
    b.raw().before(&points::UNIT_LIGHT_UPDATE, "Update the AI unit's integrated light", |call| {
        let handle = call.cpu.eax;
        let mut mem = call.ctx.runtime().memory();
        let Ok(Some((body, flags, power))) = npc_light(&*mem, handle) else {
            return Ok(Flow::Continue);
        };
        if super::converted_campaign(&*mem) {
            mem.write_bytes(body + UNIT_FLAGS, &flags.to_le_bytes(), 1)?;
            mem.write_bytes(body + LIGHT_POWER, &power.to_le_bytes(), 1)?;
        }
        // The native player-controlled update still runs.
        Ok(Flow::Continue)
    });

    b.raw().before(&points::OBJECT_FUNCTION_IMPORT, "Read the weapon's integrated light power", |call| {
        if call.arg_u32("name") != Some(INTEGRATED_LIGHT_POWER) {
            return Ok(Flow::Continue);
        }
        let Some(handle) = call.arg_u32("object") else {
            return Ok(Flow::Continue);
        };
        {
            let mut mem = call.ctx.runtime().memory();
            let Ok(Some(value)) = weapon_power(&*mem, handle) else {
                return Ok(Flow::Continue);
            };
            if !super::converted_campaign(&*mem) {
                return Ok(Flow::Continue);
            }
            mem.write_bytes(call.cpu.ebx, &value.to_le_bytes(), 1)?;
            mem.write_bytes(call.cpu.edi, &[u8::from(value > 0.0)], 1)?;
        }
        call.cpu.eax = 1;
        call.ret(8).map(Flow::Return)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{datum::layout, testing::FakeGuest, view::objects::Header};

    #[test]
    fn killed_units_fade_and_dropped_weapons_stop_using_the_former_holders_light() {
        const TABLE: u32 = 0x10000;
        const ELEMENTS: u32 = 0x20000;
        const BIPED: u32 = 0x30000;
        const RIFLE: u32 = 0x40000;
        let marine = DatumHandle::new(0x8000, 0).raw();
        let rifle = DatumHandle::new(0x8001, 1).raw();
        let mut guest = FakeGuest::new();
        guest
            .map(points::OBJECTS.base, 4)
            .map(TABLE, layout::HEADER_SIZE)
            .map(ELEMENTS, 2 * Header::SIZE)
            .map(BIPED, 0x348)
            .map(RIFLE, Body::SIZE)
            .put_u32(points::OBJECTS.base, TABLE)
            .put_u32(TABLE + layout::ELEMENT_SIZE, Header::SIZE)
            .put_u32(TABLE + layout::HIGH_WATER, 2)
            .put_u32(TABLE + layout::DATA, ELEMENTS)
            .put_u16(ELEMENTS + Header::SALT, 0x8000)
            .put_u8(ELEMENTS + Header::KIND, objects::BIPED)
            .put_u32(ELEMENTS + Header::BODY, BIPED)
            .put_u16(ELEMENTS + Header::SIZE + Header::SALT, 0x8001)
            .put_u8(ELEMENTS + Header::SIZE + Header::KIND, objects::WEAPON)
            .put_u32(ELEMENTS + Header::SIZE + Header::BODY, RIFLE)
            .put_u32(BIPED + Body::PARENT, u32::MAX)
            .put_u32(BIPED + crate::health::UNIT_PLAYER, u32::MAX)
            .put_u32(BIPED + WEAPONS, rifle)
            .put_u32(RIFLE + Body::PARENT, marine);

        set(&mut guest, marine, true).unwrap();
        assert_eq!(is_on(&guest, marine), Ok(true));
        let (_, flags, power) = npc_light(&guest, marine).unwrap().unwrap();
        assert_eq!(flags, LIGHT_ON);
        assert_eq!(power, 1.0 / 6.0);
        guest.set_f32(BIPED + LIGHT_POWER, power).unwrap();
        assert_eq!(weapon_power(&guest, rifle), Ok(Some(power)));

        guest.put_u16(BIPED + Body::DAMAGE_FLAGS, 4);
        let (_, flags, fading) = npc_light(&guest, marine).unwrap().unwrap();
        assert_eq!(flags, 0);
        assert_eq!(fading, power - 1.0 / 24.0);

        guest.put_u32(RIFLE + Body::PARENT, u32::MAX);
        assert_eq!(weapon_power(&guest, rifle), Ok(Some(0.0)));
    }
}
