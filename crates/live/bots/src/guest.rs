//! The game's memory as the bot driver uses it (docs/game/bots.md,
//! sections 2 and 4): local player slots, the Xbox Live account records,
//! and what a bot sees.
//!
//! Everything here works on [`GuestMemory`], so the tests run on synthetic
//! guest memory. Addresses are research notes; no game data is here.

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::view::{objects, players};

use crate::brain::{Me, Perception, Seen};
pub(crate) mod intel;
pub mod pickups;
pub(crate) mod round;
mod radar;

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
/// The game-time record's ticks per second (`[GAME_TIME] + 2`).
pub(crate) const GAME_TIME_RATE: u32 = 2;
/// The game-time record's tick count (`[GAME_TIME] + 8`).
pub const GAME_TIME_TICK: u32 = 8;
/// Unit `+0x168`: the aim, a world-space unit vector.
pub const UNIT_AIM: u32 = 0x168;
/// Biped flags (`+0x348`) and their facing-set bit, which the local action
/// builder's control update clears after it reseeds the look from the aim
/// (docs/game/bots.md §5.1, "The facing release").
pub(crate) const BIPED_FLAGS: u32 = guest_layout::Biped::FLAGS.offset;
pub(crate) const BIPED_FACING_SET: u8 = guest_layout::unit::FACING_SET;
/// Crouch request bit on a biped (docs/live/live-gamestate.md).
const BIPED_UNIT_FLAGS: u32 = 0x134;
const BIPED_CROUCH_REQUEST: u32 = 1 << 23;

/// `[GAME_GLOBALS] + 0x1c4`: the variant's shields, 0 normal, 1 none,
/// 2 overshields (`0x1588b0`).
const GAME_SHIELDS: u32 = 0x1c4;
/// The overshield variant's shield scale, an `f32` in the image's data.
const OVERSHIELD_SCALE: u32 = 0x0045_dbd4;
/// Player `+0xc1`: the active configuration's handicap (`+0x44 + 0x7d`),
/// 0 none to 3.
const PLAYER_HANDICAP: u32 = 0xc1;

/// The shield a spawn gives the player at `player` in a multiplayer game,
/// as `0x1588b0(player, 2)` computes it: the variant's shield scale (1,
/// 0, or `[0x45dbd4]` for overshields) times the handicap's factor (1,
/// 0.75, 0.5, 0.25, jump table `0x15897c`). Its Juggernaut branch, which
/// gives the Juggernaut the overshield scale, is not followed.
pub(crate) fn full_shield(
    m: &dyn GuestMemory,
    globals: u32,
    player: u32,
) -> Result<f32, MemFault> {
    let scale = match m.u32(globals + GAME_SHIELDS)? {
        1 => 0.0,
        2 => m.f32(OVERSHIELD_SCALE)?,
        _ => 1.0,
    };
    let factor = match m.u8(player + PLAYER_HANDICAP)? {
        1 => 0.75,
        2 => 0.5,
        3 => 0.25,
        _ => 1.0,
    };
    Ok(scale * factor)
}

/// Object body damage flags bit 2: the body is depleted (dead).
pub const BODY_DEPLETED: u16 = 1 << 2;

// ---- breakable surfaces (crate::glass) ----

/// The breakable-surface state's pointer: `[0x4ed280]`. Its first byte is
/// the enable flag `breakable_surfaces_enable` writes; from `+1` each
/// structure BSP has 32 bytes (256 bits, one a breakable index), set while
/// the surface is whole: filled with ones at map reset (`0x183ec0`) and by
/// `breakable_surfaces_reset`, a bit cleared when its surface breaks. The
/// collision query passes `[0x4ed280] + 32 * bsp + 1` as the structure's
/// mask (docs/engine/sim-rewrite-collision.md, 5.4; docs/formats/collision.md,
/// "Breakable mask").
pub const BREAKABLE_STATE: u32 = 0x004e_d280;
/// One BSP's mask, bytes.
pub const BREAKABLE_BSP_BYTES: u32 = 32;

/// The breakable-surface state of structure BSP `bsp`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breakable {
    /// The enable byte (`breakable_surfaces_enable`): with it off nothing
    /// breaks.
    pub enabled: bool,
    pub mask: crate::glass::Mask,
}

impl Breakable {
    /// The breakable indices whose surfaces are broken, ascending.
    pub fn broken(&self) -> Vec<u8> {
        (0..=255u8)
            .filter(|&i| !crate::glass::intact(&self.mask, i))
            .collect()
    }

    /// The mask's 32 bytes as the game holds them, lowest index first, in
    /// hexadecimal.
    pub fn mask_hex(&self) -> String {
        self.mask
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Reads structure BSP `bsp`'s breakable-surface state; `None` while the
/// state's pointer is null (no map).
pub fn breakable(m: &dyn GuestMemory, bsp: u32) -> R<Option<Breakable>> {
    let base = m.u32(BREAKABLE_STATE)?;
    if base == 0 {
        return Ok(None);
    }
    let at = base + 1 + BREAKABLE_BSP_BYTES * bsp;
    let mut mask = [0u32; 8];
    for (i, w) in mask.iter_mut().enumerate() {
        let mut b = [0u8; 4];
        for (k, byte) in b.iter_mut().enumerate() {
            *byte = m.u8(at + 4 * i as u32 + k as u32)?;
        }
        *w = u32::from_le_bytes(b);
    }
    Ok(Some(Breakable {
        enabled: m.u8(base)? != 0,
        mask,
    }))
}

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

/// Player `+0x194`: spawn protection. Retail sets it to the tick rate
/// at every spawn (`0x14f68e`) and subtracts one per player update
/// (`0x15bb4f`). It gives us the age of a phantom's own spawn.
pub(crate) const PLAYER_SPAWN_GRACE: u32 = 0x194;

/// `PLAYER_GLOBALS + 0xc` holds the four human local-user player datums.
pub(crate) const LOCAL_MAP: u32 = 0xc;
pub(crate) const LOCAL_USER_COUNT: u32 = 4;

/// `GAME_ENGINE + 0xe4`: four local-user opacities for the black overlay
/// drawn at `0x1599b0`. Match activation and local spawns set opacity to 1.
pub(crate) const LOCAL_BLACK_OPACITY: u32 = 0xe4;

/// Match opening on a console with local users follows the actual black
/// overlay. A dedicated host uses retail's earliest fade schedule.
pub(crate) fn match_opening_fading(m: &dyn GuestMemory, tick: u32, rate: u16) -> R<bool> {
    let users = m.u32(points::PLAYER_GLOBALS.base)?;
    let engine = m.u32(points::GAME_ENGINE.base)?;
    if users != 0 && engine != 0 {
        let mut has_local_user = false;
        for user in 0..LOCAL_USER_COUNT {
            if m.u32(users + LOCAL_MAP + 4 * user)? != u32::MAX {
                has_local_user = true;
                if m.f32(engine + LOCAL_BLACK_OPACITY + 4 * user)? > 0.0 {
                    return Ok(true);
                }
            }
        }
        if has_local_user {
            return Ok(false);
        }
    }
    // Retail starts subtracting when tick > round(1.5 * rate) at
    // 0x15c4c6. Gather sees those changes on the following tick.
    let first_update = (f32::from(rate) * 1.5).round_ties_even() as u32 + 1;
    Ok(black_fade_active(rate, tick.saturating_sub(first_update)))
}

/// Phantoms have no local opacity slot. Apply the same fade step to a
/// full black overlay for each tick since this player's spawn.
pub(crate) fn phantom_spawn_fading(m: &dyn GuestMemory, player: u32, rate: u16) -> R<bool> {
    let remaining = m.i32(player + PLAYER_SPAWN_GRACE)?;
    let age = i32::from(rate) - remaining.clamp(0, i32::from(rate));
    Ok(remaining > 0 && black_fade_active(rate, age as u32))
}

fn black_fade_active(rate: u16, updates: u32) -> bool {
    if rate == 0 {
        return false;
    }
    // Match the retail f32 subtraction at 0x15c4ea, including rounding
    // when the step is not exactly representable.
    let step = 3.0 / f32::from(rate);
    let mut opacity = 1.0;
    for _ in 0..updates {
        opacity -= step;
        if opacity <= 0.0 {
            return false;
        }
    }
    true
}

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
    perception_with(
        m,
        machine,
        sense_grenades,
        out,
        grenades,
        vehicle_headers,
        crate::sound::firing,
    )
}

/// [`perception_into`] with the players who fired lately (`firing`: the
/// slot mask at a tick) given: the motion tracker shows them as it shows
/// movement.
pub(crate) fn perception_with(
    m: &dyn GuestMemory,
    machine: [u8; 6],
    sense_grenades: bool,
    out: &mut Perception,
    grenades: &mut Vec<crate::incoming::Grenade>,
    vehicle_headers: Option<&mut Vec<crate::vehicle::read::VehicleHeader>>,
    firing: fn(u32) -> u16,
) -> R<()> {
    out.me = None;
    out.teams = false;
    out.tick = 0;
    out.objective = None;
    out.observations.player = None;
    out.observations.ahead = false;
    out.observations.service_rate = None;
    out.observations.pickups_scanned = false;
    out.observations.radar.clear();
    out.observations.radar_range = None;
    out.observations.moving = 0;
    out.observations.firing = 0;
    out.observations.ground_grenades.clear();
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
    let flags = m.u32(globals + OPTIONS + VARIANT_FLAGS)?;
    out.teams = flags & live_proto::hopper::variant::flag::TEAM_PLAY != 0;
    out.tick = game_tick(m).unwrap_or(0);
    let Some(array) = players::array(m)? else {
        out.observations.supplies = None;
        return Ok(());
    };
    let mut me_index = None;
    let sensor = radar::Sensor::read(m);
    let mut unzoomed = false;
    let mut moving = 0u16;
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
            unzoomed = m.u8(body.addr + radar::UNIT_ZOOM).is_ok_and(|level| level == u8::MAX);
            // Ammunition is extra: a fault reading it leaves the bot
            // playing without it (section 3.6), not blind.
            out.me = Some(Me {
                position,
                aim,
                team,
                shield: body.shield(m)?,
                health: body.health(m)?,
                shield_full: full_shield(m, globals, p.addr)?,
                arms: crate::arms::read(m, body.addr).ok(),
            });
        } else {
            let crouching = m.u32(body.addr + BIPED_UNIT_FLAGS)
                .is_ok_and(|flags| flags & BIPED_CROUCH_REQUEST != 0);
            if sensor.is_some_and(|s| s.moving(m, body, crouching)) {
                moving |= crate::perceive::player_bit(id);
            }
            out.observations.contacts.push(crate::brain::Contact {
                weapon: intel::held_model(m, body.addr).ok().flatten(),
                id,
                aim,
                // Only a broken shield is a visible cue. Perception removes
                // this entire contact when the bot cannot see the biped.
                shield: body.shield(m).ok().filter(|&s| s == 0.0),
                empty_magazine: false,
                crouching,
            });
            out.others.push(Seen { id, position, team });
        }
    }
    out.observations.moving = moving;
    out.observations.firing = firing(out.tick);
    if let Some(me) = me_index {
        out.observations.ahead =
            intel::ahead(m, me, out.teams, &scoreboard[..listed]).unwrap_or(false);
        let near = out.me.filter(|_| sense_grenades).map(|me| me.position);
        let mut ground = crate::arms::Ground {
            loose_grenades: std::mem::take(&mut out.observations.ground_grenades),
            weapons: std::mem::take(&mut out.pickups),
            held_power: std::mem::take(&mut out.observations.held_power),
            supplies: out.observations.supplies.take(),
            grenades: std::mem::take(grenades),
        };
        let scanned = crate::arms::ground_for(m, near, &mut ground, vehicle_headers).is_ok();
        out.observations.ground_grenades = ground.loose_grenades;
        out.pickups = ground.weapons;
        out.observations.held_power = ground.held_power;
        out.observations.supplies = ground.supplies;
        *grenades = ground.grenades;
        if scanned {
            pickups::recent_into(out.tick, &mut out.observations.takes);
            out.observations.service_rate = intel::service_rate(m);
            out.observations.pickups_scanned = true;
        } else {
            out.observations.ground_grenades.clear();
            out.pickups.clear();
            out.observations.held_power.clear();
            out.observations.supplies = None;
            grenades.clear();
        }
        // The objective is extra too: a fault leaves the bot playing as in
        // Slayer (section 3.7).
        out.objective = crate::objective::read(m, me).ok().flatten();
        // A rules script's places, which no guest memory holds: the host's
        // table (objective::script).
        out.goals = crate::objective::script::ScriptGoals {
            me,
            list: crate::objective::script::goals(),
        };
        if unzoomed && let Some(sensor) = sensor
            && radar::enabled(m, globals, flags, out.objective.as_ref())
        {
            // A firing player shows whether it moves or not, crouched too.
            let shown = moving | out.observations.firing;
            let mut dots = std::mem::take(&mut out.observations.radar);
            sensor.contacts(out, shown, &mut dots);
            out.observations.radar = dots;
            out.observations.radar_range = Some(sensor.range());
        }
    } else {
        out.observations.supplies = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
