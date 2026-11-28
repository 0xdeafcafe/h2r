//! The drop ship's guest half (docs/game/rules-world-api.md, "Vehicles and
//! drop ships"; docs/game/firefight.md, "Guest boundary"): the puts
//! of a flight and the placement of a squad into a ship.
//!
//! **Flight.** The host flies the ship along a keyed path with no AI pilot.
//! [`crate::rules::world::vehicles::Flights`] says where the ship is on
//! each tick; [`put_ship`] puts it there at the tick boundary through
//! `OBJECT_SET_TRANSFORM` (`0xb7430`) with the facing the path's tangent
//! gives. Whether the next physics step moves the body again with riders
//! aboard is R7, a lab check.
//!
//! **Boarding.** Retail squad placement cannot seat into a given ship (R6),
//! so [`place_squad_in`] places `min(n, free seats)` actors at the ship's
//! position and loads them through the engine's `vehicle_load_magic` body
//! ([`points::VEHICLE_LOAD`], `0x11b0c0`) with the squad's actor list
//! ([`points::OBJECT_LIST_FROM_AI`]). An actor the load left on foot is
//! deleted, so no surplus actor stands under the ship. [`drop_squad`] places
//! the rest of the squad on foot, as [`guest::place_squad`] does, and is the
//! whole squad's placement when the ship has no free seat or cannot load.

use guest_layout::entry::{Cx, GuestCall};
use guest_layout::view::{Datums, objects};
use guest_layout::{DatumHandle, GuestMemory, points};

use ::firefight::ShipKind;

use super::guest::{self, KitLayout, call, scenario};
use crate::actor_net::squads::Placement;

/// A flight to begin: the ship, the path's points in order, and the ticks
/// the flight takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub ship: u32,
    pub path: Vec<[f32; 3]>,
    pub ticks: u32,
}

/// One step of a flight, as an engine put.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShipPut {
    pub object: u32,
    pub at: [f32; 3],
    pub forward: [f32; 3],
}

/// The ticks a ship's flight in or out takes: 8 s at 30 ticks a second.
/// `firefight.rules` times its unload and its leaving by the same
/// (`fly_time`).
pub const FLY_TICKS: u32 = 240;

/// A kit's ship in the world: its datum and how many seats it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ship {
    pub object: u32,
    pub capacity: u8,
}

/// A zone's flight path, the kit's point set `ff_path_NN`: the ship flies
/// in from the run-in to the hover point, and out from it to the run-out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShipPath {
    pub run_in: [f32; 3],
    /// Each kind's hover point, the point set's `hover_<kind>` key
    /// ([`ShipKind::index`]); none for a kind the path is not cleared for.
    pub hovers: [Option<[f32; 3]>; ShipKind::ALL.len()],
    pub run_out: [f32; 3],
}

impl ShipPath {
    /// The points of `kind`'s flight in, none when the path has no hover
    /// key for it.
    pub fn fly_in(&self, kind: ShipKind) -> Option<Vec<[f32; 3]>> {
        Some(vec![self.run_in, self.hovers[kind.index()]?])
    }

    /// The points of `kind`'s flight out.
    pub fn fly_out(&self, kind: ShipKind) -> Option<Vec<[f32; 3]>> {
        Some(vec![self.hovers[kind.index()]?, self.run_out])
    }
}

/// The tag table global (`[4e3b44]`: 16-byte entries, the definition at
/// `+8`) and a vehicle definition's seats block count.
const TAG_TABLE: u32 = 0x004e_3b44;
const VEHICLE_SEATS: u32 = 0x1c8;

/// The seats the ship's own `vehi` tag lists, which the host counts on: an
/// engine load that seats fewer leaves the surplus on foot
/// ([`place_squad_in`]). None when the object is no live vehicle or its
/// tag is not in memory.
pub fn tag_seats(m: &dyn GuestMemory, ship: u32) -> Option<u8> {
    let (kind, body) = objects::get(m, DatumHandle::from_raw(ship)?).ok()??;
    if kind != objects::VEHICLE {
        return None;
    }
    let definition = body.definition(m).ok()??;
    let table = m.u32(TAG_TABLE).ok()?;
    let tag = m.u32(table.checked_add((definition.index() as u32) * 16 + 8)?).ok()?;
    if tag == 0 {
        return None;
    }
    u8::try_from(m.u32(tag + VEHICLE_SEATS).ok()?).ok().filter(|&n| n > 0)
}

/// The live vehicle placed at `placement` in the scenario's vehicle
/// placements, if there is exactly one.
pub fn placed_ship(m: &dyn GuestMemory, placement: i16) -> Option<u32> {
    use guest_layout::object::{Object, ObjectHeader, header::DELETE};
    let array = objects::array(m).ok()??;
    let mut found = None;
    for row in array.live_iter(m, i32::from(u16::MAX) + 1).ok()? {
        let (handle, header) = row.ok()?;
        let h = ObjectHeader::at(header.addr);
        if h.kind(m).ok()? != objects::VEHICLE || h.flags(m).ok()? & DELETE != 0 {
            continue;
        }
        let body = h.body(m).ok()?;
        if body != 0 && Object::at(body).placement_index(m).ok()? == placement {
            if found.is_some() {
                return None;
            }
            found = Some(handle.raw());
        }
    }
    found
}

/// A squad to place into a ship: the placement, the ship's datum and how
/// many seats the ship has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boarding {
    pub placement: Placement,
    pub ship: u32,
    pub capacity: u8,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `up` made perpendicular to `forward`, unit length. A flight straight up
/// or down has no usable `up`, so the world's x axis stands in.
pub fn level_up(up: [f32; 3], forward: [f32; 3]) -> [f32; 3] {
    let d = dot(up, forward);
    let u = [up[0] - forward[0] * d, up[1] - forward[1] * d, up[2] - forward[2] * d];
    let len = dot(u, u).sqrt();
    if len > 1e-4 {
        return u.map(|v| v / len);
    }
    let x = [1.0, 0.0, 0.0];
    let d = dot(x, forward);
    let u = [x[0] - forward[0] * d, x[1] - forward[1] * d, x[2] - forward[2] * d];
    let len = dot(u, u).sqrt().max(1e-4);
    u.map(|v| v / len)
}

fn up_of(m: &dyn GuestMemory, object: u32) -> Option<[f32; 3]> {
    let (_, body) = objects::get(m, DatumHandle::from_raw(object)?).ok()??;
    body.up(m).ok()
}

/// Puts the ship at its place on the path, facing the tangent. The engine
/// answers false for a position outside the world, which is the error.
pub async fn put_ship(cx: &Cx, p: ShipPut) -> anyhow::Result<()> {
    let Some(up) = cx.read(|m| up_of(m, p.object)) else {
        anyhow::bail!("ship {:#010x} is not in the world", p.object);
    };
    let up = level_up(up, p.forward);
    // The pose rides the stack under the call's own nine words, as
    // `rules::world::objects::put` passes it.
    let base = cx.entry().esp.wrapping_sub(36);
    let args = [p.object, base, base.wrapping_add(12), base.wrapping_add(24), 0, 1, 1, 0, 0]
        .into_iter()
        .chain([p.at, p.forward, up].into_iter().flatten().map(f32::to_bits));
    let r = call(cx, GuestCall::new(points::OBJECT_SET_TRANSFORM.base).args(args)).await?;
    anyhow::ensure!(
        r.eax as u8 != 0,
        "{:?} is outside the world, and the engine would not put ship {:#010x} there",
        p.at,
        p.object
    );
    Ok(())
}

/// The ship's position, if it is a live vehicle.
pub fn ship_position(m: &dyn GuestMemory, ship: u32) -> Option<[f32; 3]> {
    let (kind, body) = objects::get(m, DatumHandle::from_raw(ship)?).ok()??;
    (kind == objects::VEHICLE).then(|| body.position(m).ok()).flatten()
}

/// Seats of the ship nobody sits in: `capacity` less the units attached to
/// it. `None` when the ship is no live vehicle.
pub fn free_seats(m: &dyn GuestMemory, ship: u32, capacity: u8) -> Option<u8> {
    ship_position(m, ship)?;
    let array = objects::array(m).ok()??;
    let mut aboard = 0u32;
    for row in array.live_iter(m, i32::from(u16::MAX) + 1).ok()? {
        let (_, header) = row.ok()?;
        let body = header.body(m).ok()?;
        if body == 0 || header.kind(m).ok()? != objects::BIPED {
            continue;
        }
        if objects::Body::at(body).parent(m).ok()?.is_some_and(|p| p.raw() == ship) {
            aboard += 1;
        }
    }
    Some(u8::try_from(u32::from(capacity).saturating_sub(aboard)).unwrap_or(0))
}

/// The units of a scenario squad's live actors, in pool order. The actors
/// pool is read directly: an actor placed this boundary has no replication
/// entity yet, and `sim_state::visit_actors` skips those.
pub fn squad_units(m: &dyn GuestMemory, squad: u16) -> Vec<u32> {
    use guest_layout::actor::{Actor, CAPACITY};
    let Ok(Some(actors)) = Datums::<Actor>::from_global(m, guest_layout::global::ACTORS.base) else {
        return Vec::new();
    };
    let Ok(rows) = actors.live_iter(m, CAPACITY) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok)
        .filter(|(_, a)| a.squad(m).is_ok_and(|s| s as u16 == squad))
        .filter_map(|(_, a)| a.unit(m).ok().flatten())
        .map(|h| h.raw())
        .collect()
}

/// The squad's units not in `before`, each with whether it is attached to
/// `ship`.
pub fn new_units(m: &dyn GuestMemory, squad: u16, ship: u32, before: &[u32]) -> Vec<(u32, bool)> {
    squad_units(m, squad)
        .into_iter()
        .filter(|u| !before.contains(u))
        .map(|unit| {
            let aboard = DatumHandle::from_raw(unit)
                .and_then(|h| objects::get(m, h).ok().flatten())
                .and_then(|(_, body)| body.parent(m).ok().flatten())
                .is_some_and(|p| p.raw() == ship);
            (unit, aboard)
        })
        .collect()
}

/// Sets the slot squad's character, zone and starts in the scenario, every
/// start at `at` with the zone's first start's reference frame.
fn stage(cx: &Cx, kit: &KitLayout, p: &Placement, squad: u16, at: [f32; 3]) -> anyhow::Result<()> {
    let zone = kit
        .zones
        .get(usize::from(p.zone))
        .ok_or_else(|| anyhow::anyhow!("the kit has no zone {}", p.zone))?;
    let frame = zone
        .starts
        .first()
        .map(|s| s.1)
        .ok_or_else(|| anyhow::anyhow!("kit zone {} has no firing positions", p.zone))?;
    anyhow::ensure!(
        p.character < kit.characters,
        "character {} is outside the map's {} palette rows",
        p.character,
        kit.characters
    );
    cx.read(|m| guest::character_ready(m, p.character))?;
    cx.write(|m| {
        let scnr = m.u32(points::SCENARIO.base)?;
        let rec = *guest::block(m, scnr, scenario::SQUADS)?
            .get(usize::from(squad))
            .ok_or(guest_layout::MemFault {
                addr: scnr + scenario::SQUADS.0,
                len: 4,
                write: false,
            })?;
        m.set_u16(rec + scenario::SQUAD_CHARACTER, p.character)?;
        m.set_u16(rec + scenario::SQUAD_ZONE, zone.index)?;
        for start in guest::block(m, rec, scenario::SQUAD_STARTS)? {
            for (k, v) in at.into_iter().enumerate() {
                m.set_f32(start + scenario::START_POSITION + 4 * k as u32, v)?;
            }
            m.set_u16(start + scenario::START_REFERENCE_FRAME, frame)?;
        }
        Ok::<_, guest_layout::MemFault>(())
    })
    .map_err(guest::guest)?;
    Ok(())
}

/// Places `min(count, free seats)` actors of the placement's squad at the
/// ship and loads them into it. Returns how many are aboard. Refused, with
/// no actor left standing, when none fit or the engine seats none. An
/// actor the load left on foot is deleted.
pub async fn place_squad_in(cx: &Cx, kit: &KitLayout, b: Boarding) -> anyhow::Result<u8> {
    let p = b.placement;
    let squad = guest::squad_index(kit, p.slot)?;
    let (free, at, before) = cx.read(|m| {
        (
            free_seats(m, b.ship, b.capacity),
            ship_position(m, b.ship),
            squad_units(m, squad),
        )
    });
    let (Some(free), Some(at)) = (free, at) else {
        anyhow::bail!("ship {:#010x} is not a live vehicle", b.ship);
    };
    let seats = u16::from(free).min(p.count);
    anyhow::ensure!(seats > 0, "ship {:#010x} has no free seat", b.ship);
    stage(cx, kit, &p, squad, at)?;
    call(
        cx,
        GuestCall::new(points::AI_SQUAD_PLACE.base)
            .eax(u32::from(seats))
            .args([u32::from(squad), u32::MAX, 0, 1]),
    )
    .await?;
    call(
        cx,
        GuestCall::new(points::AI_SET_TEAM.base)
            .eax(u32::from(squad))
            .ebx(u32::from(p.team)),
    )
    .await?;
    // Load what was placed. A failed load still reaches the cleanup below.
    let loaded = load(cx, b.ship, squad).await;
    let placed = cx.read(|m| new_units(m, squad, b.ship, &before));
    for &(unit, _) in placed.iter().filter(|(_, aboard)| !aboard) {
        call(cx, GuestCall::new(points::OBJECT_DELETE.base).args([unit])).await?;
    }
    loaded?;
    let seated = placed.iter().filter(|(_, aboard)| *aboard).count();
    anyhow::ensure!(seated > 0, "the engine seated none of the squad in ship {:#010x}", b.ship);
    Ok(u8::try_from(seated).unwrap_or(u8::MAX))
}

/// `vehicle_load_magic` of the squad's actors into the ship, every seat.
async fn load(cx: &Cx, ship: u32, squad: u16) -> anyhow::Result<u16> {
    let list = call(
        cx,
        GuestCall::new(points::OBJECT_LIST_FROM_AI.base).args([u32::from(squad)]),
    )
    .await?
    .eax;
    anyhow::ensure!(list != u32::MAX, "the engine made no object list of squad {squad}");
    let r = call(
        cx,
        GuestCall::new(points::VEHICLE_LOAD.base)
            .edx(ship)
            .args([u32::MAX, list]),
    )
    .await?;
    Ok(r.eax as u16)
}

/// The ship's hover hold, as the rules runtime's `hover` makes it
/// (`VEHICLE_HOVER`, `0xf0fd0`): refused for a ship no longer in the world.
pub async fn hover(cx: &Cx, ship: u32, on: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        cx.read(|m| ship_position(m, ship).is_some()),
        "ship {ship:#010x} is not a live vehicle"
    );
    call(
        cx,
        GuestCall::new(points::VEHICLE_HOVER.base).eax(ship).ecx(u32::from(on)),
    )
    .await?;
    Ok(())
}

/// Everyone aboard leaves, every seat (`VEHICLE_UNLOAD`, `0x11b2b0`).
pub async fn unload(cx: &Cx, ship: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        cx.read(|m| ship_position(m, ship).is_some()),
        "ship {ship:#010x} is not a live vehicle"
    );
    call(
        cx,
        GuestCall::new(points::VEHICLE_UNLOAD.base).edx(ship).ecx(u32::MAX),
    )
    .await?;
    Ok(())
}

/// Places the whole placement: up to the free seats into the ship, the rest
/// on foot at the zone as [`guest::place_squad`] does. A ship that cannot
/// take the squad (no free seat, not in the world, a load the engine
/// refuses) leaves the whole squad on foot; the reason is printed. Returns
/// how many are aboard.
pub async fn drop_squad(cx: &Cx, kit: &KitLayout, b: Boarding) -> anyhow::Result<u8> {
    let seated = match place_squad_in(cx, kit, b).await {
        Ok(n) => n,
        Err(why) => {
            eprintln!("firefight: squad {} lands on foot: {why}", b.placement.slot);
            0
        }
    };
    let rest = b.placement.count.saturating_sub(u16::from(seated));
    if rest > 0 {
        guest::place_squad(
            cx,
            kit,
            Placement {
                count: rest,
                ..b.placement
            },
        )
        .await?;
    }
    Ok(seated)
}

#[cfg(test)]
mod tests;
