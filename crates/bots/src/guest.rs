//! The game's memory as the bot driver uses it (docs/game/bots.md,
//! sections 2 and 4): local player slots, the Xbox Live account records,
//! and what a bot sees.
//!
//! Everything here works on [`GuestMemory`], so the tests run on synthetic
//! guest memory. Addresses are research notes; no game data is here.

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use engine::view::{objects, players};

use crate::brain::{Me, Perception, Seen};
pub(crate) mod intel;
pub mod pickups;

pub type R<T> = Result<T, MemFault>;

// ---- player profiles ----

/// The four local player slots, one per controller port.
pub const SLOTS: u32 = 0x0054_e8e0;
pub const SLOT_STRIDE: u32 = 0xc70;
/// Slot `+0` bit: a profile is signed in (`0x18fb34` sets it).
pub const SLOT_PROFILE: u32 = 0x10;
/// The bot's look sensitivity, which the brain's turning is tuned to.
pub const BOT_SENSITIVITY: u8 = 3;
/// The eight checked profile bytes. The primary layer shares the primary
/// armour colour; the other three masks occupy the remaining colour slots.
pub fn bot_look(name: &str) -> [u8; 8] {
    let (e, c) = live_proto::bots::appearance(name);
    [
        c.primary,
        c.secondary,
        e.colors[1],
        e.colors[2],
        e.foreground & 1, // Spartan or Elite, stable for the normalized gamertag.
        e.foreground,
        e.background,
        0,
    ]
}

pub fn slot(port: u32) -> u32 {
    SLOTS + port * SLOT_STRIDE
}

/// Whether `port`'s slot has a profile signed in.
pub fn signed_in(m: &dyn GuestMemory, port: u32) -> R<bool> {
    Ok(m.u32(slot(port))? & SLOT_PROFILE != 0)
}

/// `name` as the block's profile name: UTF-16, at most 31 units, the rest
/// NUL.
pub fn profile_name_bytes(name: &str) -> [u8; 64] {
    const BLOCK_PROFILE_NAME_UNITS: usize = 32;
    let mut out = [0u8; BLOCK_PROFILE_NAME_UNITS * 2];
    for (dst, u) in out
        .chunks_exact_mut(2)
        .zip(name.encode_utf16().take(BLOCK_PROFILE_NAME_UNITS - 1))
    {
        dst.copy_from_slice(&u.to_le_bytes());
    }
    out
}

/// Slot `+4`: the slot's local user, or -1.
pub const SLOT_LOCAL_USER: u32 = 4;
/// Size of the session player ID (XUID and user flags).
pub const ID_LEN: usize = 12;

// ---- what a bot sees (section 3.1, stage 1) ----

/// Game options: `[GAME_GLOBALS] + 8`; `+0` the mode (2 multiplayer), the
/// variant at `+0x134`, its flags at `+0x48` (bit 0 a team game). As Live
/// telemetry reads them.
pub const OPTIONS: u32 = 8;
pub const MODE_MULTIPLAYER: u32 = 2;
pub const VARIANT_FLAGS: u32 = 0x134 + 0x48;
/// The game-time record's tick count (`[GAME_TIME] + 8`).
pub const GAME_TIME_TICK: u32 = 8;
/// Unit `+0x168`: the aim, a world-space unit vector.
pub const UNIT_AIM: u32 = 0x168;
/// Crouch request bit on a biped (docs/live/live-gamestate.md).
const BIPED_UNIT_FLAGS: u32 = 0x134;
const BIPED_CROUCH_REQUEST: u32 = 1 << 23;

/// Object body damage flags bit 2: the body is depleted (dead).
pub const BODY_DEPLETED: u16 = 1 << 2;

/// A fast same-tick check before reusing a phantom's sampled pad.
pub(crate) fn same_live_unit(m: &dyn GuestMemory, machine: [u8; 6], slot: u32, unit: u32) -> bool {
    let check = || -> R<bool> {
        if game_mode(m) != Some(MODE_MULTIPLAYER) {
            return Ok(false);
        }
        let Some(array) = players::array(m)? else {
            return Ok(false);
        };
        let Some(p) = array.get_absolute(m, slot as i32)? else {
            return Ok(false);
        };
        if !p.listed(m)? || !is(m, &p, machine)? {
            return Ok(false);
        }
        let Some(handle) = p.unit(m)?.filter(|h| h.raw() == unit) else {
            return Ok(false);
        };
        let Some((kind, body)) = objects::get(m, handle)? else {
            return Ok(false);
        };
        Ok(kind == objects::BIPED && body.damage_flags(m)? & BODY_DEPLETED == 0)
    };
    check().unwrap_or(false)
}

/// A player's living unit: its origin (in the world: a rider's is its
/// vehicle's), aim and body.
fn living_unit(
    m: &dyn GuestMemory,
    p: &players::Player,
) -> R<Option<([f32; 3], [f32; 3], objects::Body)>> {
    let Some(unit) = p.unit(m)? else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(m, unit)? else {
        return Ok(None);
    };
    if kind != objects::BIPED || body.damage_flags(m)? & BODY_DEPLETED != 0 {
        return Ok(None);
    }
    // In a vehicle the unit's own position is relative to it
    // (docs/game/bots.md section 3.9): the vehicle's is where it is.
    Ok(Some((
        crate::vehicle::read::unit_position(m, body)?,
        m.vec3(body.addr + UNIT_AIM)?,
        body,
    )))
}

/// The game's tick count (30 a second) while a game is loaded, the main
/// menu's included: `mainmenu.map` runs as a game of its own (options
/// mode 3) with a game-time record that ticks. `None` while no game
/// is loaded at all.
pub fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    let time = m.u32(points::GAME_TIME.base).ok().filter(|&t| t != 0)?;
    m.u32(time + GAME_TIME_TICK).ok()
}

/// The loaded game's options mode (`[GAME_GLOBALS] + 8`), if readable.
pub fn game_mode(m: &dyn GuestMemory) -> Option<u32> {
    let g = m.u32(points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    m.u32(g + OPTIONS).ok()
}

/// Player `+0x14`: the player's machine id, 6 bytes.
pub const PLAYER_MACHINE: u32 = 0x14;

/// Player `p`'s machine id.
pub fn player_machine(m: &dyn GuestMemory, p: u32) -> R<[u8; 6]> {
    let mut id = [0; 6];
    for (i, b) in (0u32..).zip(id.iter_mut()) {
        *b = m.u8(p + PLAYER_MACHINE + i)?;
    }
    Ok(id)
}

/// Whether player `p` is the phantom on this machine.
pub(crate) fn is(m: &dyn GuestMemory, p: &players::Player, machine: [u8; 6]) -> R<bool> {
    Ok(player_machine(m, p.addr)? == machine)
}

/// What the phantom sees in a multiplayer game: itself and the other
/// living players, available weapons and the objective.
pub fn perceive(m: &dyn GuestMemory, machine: [u8; 6]) -> R<Perception> {
    let mut out = Perception::default();
    perception_into(m, machine, false, &mut out, &mut Vec::new(), None)?;
    Ok(out)
}

#[cfg(test)]
pub(crate) fn perceive_for_brain(
    m: &dyn GuestMemory,
    machine: [u8; 6],
) -> R<(Perception, Vec<crate::incoming::Grenade>)> {
    let mut out = Perception::default();
    let mut grenades = Vec::new();
    perception_into(m, machine, true, &mut out, &mut grenades, None)?;
    Ok((out, grenades))
}

pub(crate) fn perception_into(
    m: &dyn GuestMemory,
    machine: [u8; 6],
    sense_grenades: bool,
    out: &mut Perception,
    grenades: &mut Vec<crate::incoming::Grenade>,
    vehicle_headers: Option<&mut Vec<crate::vehicle::read::VehicleHeader>>,
) -> R<()> {
    out.me = None;
    out.teams = false;
    out.tick = 0;
    out.objective = None;
    out.observations.player = None;
    out.observations.ahead = false;
    out.observations.service_rate = None;
    out.observations.pickups_scanned = false;
    out.observations.contacts.clear();
    out.observations.supply_players.clear();
    out.observations.held_power.clear();
    out.observations.takes.clear();
    out.others.clear();
    out.pickups.clear();
    grenades.clear();
    if let Some(sites) = &mut out.observations.supplies {
        sites.clear();
    }
    let globals = m.u32(points::GAME_GLOBALS.base)?;
    if globals == 0 || m.u32(globals + OPTIONS)? != MODE_MULTIPLAYER {
        out.observations.supplies = None;
        return Ok(());
    }
    out.teams = m.u32(globals + OPTIONS + VARIANT_FLAGS)? & 1 != 0;
    out.tick = game_tick(m).unwrap_or(0);
    let Some(array) = players::array(m)? else {
        out.observations.supplies = None;
        return Ok(());
    };
    let mut me_index = None;
    let mut scoreboard = [(0, 0); 16];
    let mut listed = 0;
    for player in array.live_iter(m, 16)? {
        let (handle, p) = player?;
        if !p.listed(m)? {
            continue;
        }
        let id = u32::from(handle.index());
        let team = i16::from(p.team(m)?);
        scoreboard[listed] = (id, team);
        listed += 1;
        let ours = is(m, &p, machine)?;
        if ours {
            out.observations.player = Some((id, team));
        }
        let Some((position, aim, body)) = living_unit(m, &p)? else {
            continue;
        };
        let hand = |offset| {
            let slot = u32::from(m.u8(body.addr + offset).ok()?);
            if slot >= crate::arms::UNIT_WEAPON_SLOTS {
                return None;
            }
            DatumHandle::from_raw(
                m.u32(body.addr + crate::arms::UNIT_WEAPONS + slot * 4)
                    .ok()?,
            )
            .map(|h| h.raw())
        };
        if let Some(unit) = p.unit(m)? {
            out.observations
                .supply_players
                .push(crate::decisions::supplies::Player {
                    id,
                    unit: unit.raw(),
                    position,
                    overshield: body.shield(m).is_ok_and(|shield| shield > 1.0)
                        || m.u8(body.addr + 0x10a).is_ok_and(|f| f & 0x10 != 0),
                    camo: m.u16(p.addr + 0x198).is_ok_and(|t| (t as i16) > 0),
                    hands: [
                        hand(crate::arms::UNIT_WEAPON_INDEX),
                        hand(crate::arms::UNIT_LEFT_WEAPON_INDEX),
                    ],
                });
        }
        if ours {
            me_index = Some(id);
            // Ammunition is extra: a fault reading it leaves the bot
            // playing without it (section 3.6), not blind.
            out.me = Some(Me {
                position,
                aim,
                team,
                shield: body.shield(m)?,
                health: body.health(m)?,
                arms: crate::arms::read(m, body.addr).ok(),
            });
        } else {
            out.observations.contacts.push(crate::brain::Contact {
                weapon: intel::held_model(m, body.addr).ok().flatten(),
                id,
                aim,
                // These cues require a visible effect or reload animation.
                shield: None,
                empty_magazine: false,
                crouching: m
                    .u32(body.addr + BIPED_UNIT_FLAGS)
                    .is_ok_and(|flags| flags & BIPED_CROUCH_REQUEST != 0),
            });
            out.others.push(Seen { id, position, team });
        }
    }
    if let Some(me) = me_index {
        out.observations.ahead =
            intel::ahead(m, me, out.teams, &scoreboard[..listed]).unwrap_or(false);
        let near = out.me.filter(|_| sense_grenades).map(|me| me.position);
        let mut ground = crate::arms::Ground {
            weapons: std::mem::take(&mut out.pickups),
            held_power: std::mem::take(&mut out.observations.held_power),
            supplies: out.observations.supplies.take(),
            grenades: std::mem::take(grenades),
        };
        let scanned = crate::arms::ground_into(m, near, &mut ground, vehicle_headers).is_ok();
        out.pickups = ground.weapons;
        out.observations.held_power = ground.held_power;
        out.observations.supplies = ground.supplies;
        *grenades = ground.grenades;
        if scanned {
            pickups::recent_into(out.tick, &mut out.observations.takes);
            out.observations.service_rate = intel::service_rate(m);
            out.observations.pickups_scanned = true;
        } else {
            out.pickups.clear();
            out.observations.held_power.clear();
            out.observations.supplies = None;
            grenades.clear();
        }
        // The objective is extra too: a fault leaves the bot playing as in
        // Slayer (section 3.7).
        out.objective = crate::objective::read(m, me).ok().flatten();
    } else {
        out.observations.supplies = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
