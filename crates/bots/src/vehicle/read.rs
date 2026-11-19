//! Vehicles in guest memory (docs/game/bots.md, section 3.8, "Fields"):
//! the vehicle objects, their seats from the `vehi` tag, and who sits
//! where. Everything here works on [`GuestMemory`], so the tests run on
//! synthetic guest memory.
//!
//! - object `+0x14` the parent (a rider's vehicle, a turret's mount),
//!   `+0x64`/`+0x70`/`+0x7c` position, forward and up (relative to the
//!   parent when there is one), `+0xec` body vitality, `+0x10a` damage
//!   flags (bit 2 depleted);
//! - unit `+0x1fc` the seat (`i16`, -1 none);
//! - `vehi` `+0x1f0` the vehicle type (`u16`), `+0x1c8`/`+0x1cc` the seats
//!   block (`0xb0` each), a seat's `+0` its flags (bit 2 driver, bit 3
//!   gunner).

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::tags::GroupTag;
use engine::view::{Tags, objects, players};

use super::{Class, Rider, Riding, Seat, SeatRole, Sight, Vehicle};
use crate::brain::Vec3;
use crate::guest::{BODY_DEPLETED, Who};

type R<T> = Result<T, MemFault>;

/// Unit `+0x1fc`: the seat in the vehicle it rides (`i16`, -1 none).
/// Confirmed (`crates/live-telemetry/src/layout.rs`, `UNIT_SEAT`): the
/// enter-vehicle routine `0xea1f0` stores it (`0xea405`), leaving stores
/// -1 (`0xea995`).
pub const UNIT_SEAT: u32 = 0x1fc;
/// `vehi` `+0x1f0`: the vehicle type (`u16`; 5 and 6 are compared by the
/// vehicle code, `0xef545`: the fighter and the turret).
pub const VEHI_TYPE: u32 = 0x1f0;
/// `vehi` `+0x1c8`: the seats block's count, then its pointer.
pub const VEHI_SEATS: u32 = 0x1c8;
/// A seat definition's size (`0xea330`: `imul edi,0xb0`).
pub const SEAT_STRIDE: u32 = 0xb0;
/// A seat definition's `+0`: its flags (`u32`).
pub const SEAT_FLAGS: u32 = 0;
pub const SEAT_DRIVER: u32 = 1 << 2;
pub const SEAT_GUNNER: u32 = 1 << 3;
/// Seats looked at per vehicle, at most.
pub const MAX_SEATS: i32 = 16;
/// Vehicles looked at, at most (the objects array's slots).
pub const OBJECT_LIMIT: i32 = 2048;

const VEHI: GroupTag = GroupTag::from_chars(*b"vehi");

/// A vehicle's tag: its class and its seats' roles.
fn definition(m: &dyn GuestMemory, def: DatumHandle) -> R<Option<(Class, Vec<SeatRole>)>> {
    let Some(tag) = Tags.entry(m, def)? else {
        return Ok(None);
    };
    if tag.group(m)? != VEHI {
        return Ok(None);
    }
    let vehi = tag.address(m)?;
    if vehi == 0 {
        return Ok(None);
    }
    let class = Class::from_type(m.u16(vehi + VEHI_TYPE)?);
    let count = m.i32(vehi + VEHI_SEATS)?.clamp(0, MAX_SEATS) as u32;
    let base = if count > 0 {
        m.u32(vehi + VEHI_SEATS + 4)?
    } else {
        0
    };
    let mut flags = Vec::with_capacity(count as usize);
    for i in 0..count {
        flags.push(m.u32(base + i * SEAT_STRIDE + SEAT_FLAGS)?);
    }
    Ok(Some((class, roles(&flags))))
}

/// The seats' roles from their flags. With no driver's seat flagged at
/// all, the first seat that is not a gunner's drives (a guess for a tag
/// read wrong rather than a vehicle nobody drives).
pub fn roles(flags: &[u32]) -> Vec<SeatRole> {
    let mut out: Vec<SeatRole> = flags
        .iter()
        .map(|f| {
            if f & SEAT_DRIVER != 0 {
                SeatRole::Driver
            } else if f & SEAT_GUNNER != 0 {
                SeatRole::Gunner
            } else {
                SeatRole::Passenger
            }
        })
        .collect();
    if !out.contains(&SeatRole::Driver)
        && let Some(s) = out.iter_mut().find(|r| **r == SeatRole::Passenger)
    {
        *s = SeatRole::Driver;
    }
    out
}

/// An object's own pose: position, forward, up.
fn pose(m: &dyn GuestMemory, body: objects::Body) -> R<[Vec3; 3]> {
    Ok([body.position(m)?, body.forward(m)?, body.up(m)?])
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `local` (a pose relative to `parent`) in the world: the parent's
/// forward, left (up cross forward) and up are the local X, Y and Z.
pub fn compose(parent: [Vec3; 3], local: [Vec3; 3]) -> [Vec3; 3] {
    let [p, f, u] = parent;
    let l = cross(u, f);
    let dir = |v: Vec3| {
        [
            f[0] * v[0] + l[0] * v[1] + u[0] * v[2],
            f[1] * v[0] + l[1] * v[1] + u[1] * v[2],
            f[2] * v[0] + l[2] * v[1] + u[2] * v[2],
        ]
    };
    let at = dir(local[0]);
    [
        [p[0] + at[0], p[1] + at[1], p[2] + at[2]],
        dir(local[1]),
        dir(local[2]),
    ]
}

/// The vehicle object `h` names, if it is a live vehicle: its body.
fn vehicle_body(m: &dyn GuestMemory, h: Option<DatumHandle>) -> R<Option<objects::Body>> {
    let Some(h) = h else { return Ok(None) };
    Ok(match objects::get(m, h)? {
        Some((objects::VEHICLE, body)) => Some(body),
        _ => None,
    })
}

/// A vehicle's pose in the world: its own, under each vehicle it is
/// mounted on (a turret's is relative to its mount).
fn world_pose(m: &dyn GuestMemory, body: objects::Body) -> R<[Vec3; 3]> {
    let mut out = pose(m, body)?;
    let mut at = body;
    for _ in 0..4 {
        let Some(parent) = vehicle_body(m, at.parent(m)?)? else {
            break;
        };
        out = compose(pose(m, parent)?, out);
        at = parent;
    }
    Ok(out)
}

/// Where the unit whose body is `body` is in the world: in a vehicle, the
/// vehicle's position (a rider's own `+0x64` is relative to it, as Live
/// telemetry reads riders); on foot, its own.
pub fn unit_position(m: &dyn GuestMemory, body: objects::Body) -> R<Vec3> {
    match vehicle_body(m, body.parent(m)?)? {
        Some(v) => Ok(world_pose(m, v)?[0]),
        None => body.position(m),
    }
}

/// What the bot `who` knows about vehicles: every vehicle in the world
/// that is not wrecked, who sits in each seat, and its own seat. Outside a
/// game, or with no objects, nothing.
pub fn sight(m: &dyn GuestMemory, who: Who) -> R<Sight> {
    let mut out = Sight::default();
    let Some(array) = objects::array(m)? else {
        return Ok(out);
    };
    for (h, header) in array.live(m, OBJECT_LIMIT)? {
        if header.kind(m)? != objects::VEHICLE || header.body(m)? == 0 {
            continue;
        }
        let body = objects::Body::at(header.body(m)?);
        if body.damage_flags(m)? & BODY_DEPLETED != 0 {
            continue;
        }
        let Some(def) = body.definition(m)? else {
            continue;
        };
        // A vehicle whose tag does not read is no use to a bot.
        let Some((class, roles)) = definition(m, def).ok().flatten() else {
            continue;
        };
        let [position, forward, up] = world_pose(m, body)?;
        let on = vehicle_body(m, body.parent(m)?)?
            .and(body.parent(m)?)
            .map(DatumHandle::raw);
        out.vehicles.push(Vehicle {
            id: h.raw(),
            class,
            position,
            forward,
            up,
            health: body.health(m)?,
            on,
            seats: roles
                .into_iter()
                .map(|role| Seat { role, rider: None })
                .collect(),
        });
    }
    if out.vehicles.is_empty() {
        return Ok(out);
    }
    let Some(players) = players::array(m)? else {
        return Ok(out);
    };
    for (handle, p) in players.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        let Some(unit) = p.unit(m)? else { continue };
        let Some((kind, body)) = objects::get(m, unit)? else {
            continue;
        };
        if kind != objects::BIPED || body.damage_flags(m)? & BODY_DEPLETED != 0 {
            continue;
        }
        let Some(parent) = body.parent(m)? else {
            continue;
        };
        let seat = m.u16(body.addr + UNIT_SEAT)? as i16;
        let Some(v) = out.vehicles.iter_mut().find(|v| v.id == parent.raw()) else {
            continue;
        };
        let Some(s) = usize::try_from(seat).ok().and_then(|i| v.seats.get_mut(i)) else {
            continue;
        };
        s.rider = Some(Rider {
            player: u32::from(handle.index()),
            team: i16::from(p.team(m)?),
        });
        if crate::guest::is(m, &p, who)? {
            out.riding = Some(Riding {
                vehicle: v.id,
                seat: seat as u16,
                role: s.role,
            });
        }
    }
    Ok(out)
}

/// [`sight`] in a multiplayer game only (as [`crate::guest::perceive_as`]
/// gates it), and nothing on a fault: vehicles are extra, a bot that
/// cannot read them plays on foot.
pub fn sight_in_game(m: &dyn GuestMemory, who: Who) -> Sight {
    let multiplayer = m
        .u32(points::GAME_GLOBALS.base)
        .ok()
        .filter(|&g| g != 0)
        .and_then(|g| m.u32(g + crate::guest::OPTIONS).ok())
        == Some(crate::guest::MODE_MULTIPLAYER);
    if !multiplayer {
        return Sight::default();
    }
    sight(m, who).unwrap_or_default()
}

#[cfg(test)]
mod tests;
