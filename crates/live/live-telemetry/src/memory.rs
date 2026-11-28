mod film;
pub use film::{FilmSceneReader, film_scene};

use live_proto::game::{Player, TeamScore};
use runtime::{Guest, GuestFault};

use crate::event::{
    ActorState, Animation, Appearance, CharacterContact, GameInfo, HitRecord, ItemState,
    MachineState, Motion, ObjectState, PlacedObject, Placements, PlayerState, PlayerStats, Pose,
    ProjectileState, Snapshot, Support, VehicleState, Weapon, World,
};
use crate::layout;
use guest_layout::DatumHandle;
use guest_layout::view::{Datums, players};
use live_proto::game::ObjectKind;

pub type ReadResult<T> = Result<T, GuestFault>;

fn i16(v: u16) -> i16 {
    v as i16
}

fn i32(v: u32) -> i32 {
    v as i32
}

fn nonzero(value: i32) -> Option<i64> {
    (value != 0).then_some(i64::from(value))
}

fn ptr(guest: &Guest, address: u32) -> ReadResult<u32> {
    let p = guest.u32(address)?;
    if p == 0 {
        return Err(GuestFault {
            addr: address,
            len: 4,
            access: runtime::guest::Access::Read,
            reason: runtime::guest::FaultReason::Unmapped,
        });
    }
    Ok(p)
}

fn text(guest: &Guest, address: u32, capacity: usize) -> ReadResult<String> {
    let mut bytes = vec![0; capacity];
    guest.read_bytes(address, &mut bytes, 1)?;
    let len = bytes.iter().position(|b| *b == 0).unwrap_or(capacity);
    Ok(String::from_utf8_lossy(&bytes[..len]).into_owned())
}

/// The loaded map's short name, from the cache header (`lockout`).
pub fn loaded_map_name(guest: &Guest) -> ReadResult<String> {
    text(guest, layout::CACHE_MAP_NAME, 32)
}

pub fn game_tick(guest: &Guest) -> ReadResult<u64> {
    let time = ptr(guest, layout::GAME_TIME_PTR)?;
    Ok(u64::from(guest.u32(time + layout::TIME_TICK)?))
}

pub fn game_info(guest: &Guest) -> ReadResult<GameInfo> {
    let globals = ptr(guest, layout::GAME_GLOBALS_PTR)?;
    let options = globals + layout::OPTIONS;
    let variant = options + layout::VARIANT;
    let time = ptr(guest, layout::GAME_TIME_PTR)?;
    let mode = match guest.u32(options + layout::OPTIONS_TYPE)? {
        1 => "campaign",
        2 => "multiplayer",
        3 => "main_menu",
        other => {
            return Ok(GameInfo {
                map: text(guest, layout::CACHE_MAP_NAME, 32)?,
                mode: format!("unknown_{other}"),
                difficulty: None,
                skulls: None,
                variant: None,
                team_play: false,
                score_limit: None,
                time_limit_s: None,
                tick_rate: 30,
                tick: u64::from(guest.u32(time + layout::TIME_TICK)?),
            });
        }
    };
    let engine = i32(guest.u32(variant + layout::VARIANT_ENGINE)?);
    let variant_name = layout::variant_name(engine)
        .map(str::to_owned)
        .or_else(|| (engine > 0).then(|| format!("engine_{engine}")));
    let difficulty = if mode == "campaign" {
        let value = guest.u16(options + layout::OPTIONS_DIFFICULTY)? as i16;
        layout::difficulty_name(value).map(str::to_owned)
    } else {
        None
    };
    // Every skull reader in the game tests for a campaign first
    // (docs/game/skulls.md "In multiplayer"), so the array says what is on only
    // in one; a multiplayer game's skulls are the host's choice, which the
    // boot hands to `Telemetry::set_multiplayer_skulls`.
    let skulls = if mode == "campaign" {
        let mut on = [0u8; layout::SKULL_COUNT];
        // Unreadable says nothing rather than failing the whole start.
        guest
            .read_bytes(layout::SKULLS, &mut on, 1)
            .ok()
            .and_then(|_| live_proto::skulls::from_flags(&on.map(|b| b != 0)))
    } else {
        None
    };
    Ok(GameInfo {
        map: text(guest, layout::CACHE_MAP_NAME, 32)?,
        mode: mode.into(),
        difficulty,
        skulls,
        variant: variant_name,
        team_play: guest.u32(variant + layout::VARIANT_FLAGS)? & 1 != 0,
        score_limit: nonzero(i32(guest.u32(variant + layout::VARIANT_SCORE_LIMIT)?)),
        time_limit_s: nonzero(i32(guest.u32(variant + layout::VARIANT_TIME_LIMIT)?)),
        tick_rate: guest.u16(options + layout::OPTIONS_TICK_RATE)?.max(1),
        tick: u64::from(guest.u32(time + layout::TIME_TICK)?),
    })
}

/// Reads a local player's appearance from their player slot's signed-in
/// profile copy (docs/live/live-gamestate.md, "Local player slots and profile
/// appearance"). Only the four local slots hold profiles; a remote player or
/// a slot without the signed-in flag reads as empty. Values outside their
/// known ranges are dropped so an unvalidated "likely" offset cannot put
/// garbage in events.
fn appearance(guest: &Guest, local_user: i16) -> ReadResult<Appearance> {
    if !(0..layout::PLAYER_SLOT_COUNT as i16).contains(&local_user) {
        return Ok(Appearance::default());
    }
    let slot = layout::PLAYER_SLOTS + local_user as u32 * layout::PLAYER_SLOT_SIZE;
    if guest.u32(slot + layout::PLAYER_SLOT_FLAGS)? & layout::PLAYER_SLOT_SIGNED_IN == 0 {
        return Ok(Appearance::default());
    }
    let block = slot + layout::PLAYER_SLOT_PROFILE + layout::PROFILE_APPEARANCE;
    let field = |offset: u32, limit: u32| -> ReadResult<Option<u8>> {
        let value = guest.u8(block + offset)?;
        Ok((u32::from(value) < limit).then_some(value))
    };
    Ok(Appearance {
        primary: field(layout::APPEARANCE_PRIMARY, layout::PALETTE_COLOURS)?,
        secondary: field(layout::APPEARANCE_SECONDARY, layout::PALETTE_COLOURS)?,
        tertiary: field(layout::APPEARANCE_TERTIARY, layout::PALETTE_COLOURS)?,
        quaternary: field(layout::APPEARANCE_QUATERNARY, layout::PALETTE_COLOURS)?,
        emblem_foreground: field(
            layout::APPEARANCE_EMBLEM_FOREGROUND,
            layout::EMBLEM_FOREGROUNDS,
        )?,
        emblem_background: field(
            layout::APPEARANCE_EMBLEM_BACKGROUND,
            layout::EMBLEM_BACKGROUNDS,
        )?,
    })
}

/// A view's read fault as the runtime's (views report no reason).
fn fault(f: guest_layout::MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write {
            runtime::guest::Access::Write
        } else {
            runtime::guest::Access::Read
        },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

pub fn players(guest: &Guest) -> ReadResult<Vec<PlayerState>> {
    players_after(guest, &mut [])
}

/// [`players`], taking each name from `previous` (the last read's roster)
/// when the player's datum and name are unchanged, so a steady roster
/// decodes no names.
pub fn players_after(guest: &Guest, previous: &mut [PlayerState]) -> ReadResult<Vec<PlayerState>> {
    let mut out = Vec::with_capacity(previous.len());
    players_into(guest, previous, &mut out)?;
    Ok(out)
}

/// [`players_after`] into `out` (cleared first), reusing its allocation.
fn players_into(
    guest: &Guest,
    previous: &mut [PlayerState],
    out: &mut Vec<PlayerState>,
) -> ReadResult<()> {
    out.clear();
    let array = ptr(guest, layout::PLAYERS_PTR)?;
    // A null data pointer is an error, as a null array is.
    ptr(guest, array + layout::DATUM_DATA)?;
    let players: Datums<players::Player> = Datums::at(array);
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    let statborg = engine + layout::ENGINE_STATBORG;
    let has_stats = guest.u32(statborg)? != 0;
    let mut units = [0; players::NAME_UNITS];
    // `Datums::live`'s walk (salt non-zero below the high-water mark, at
    // most 16 slots) without its list: this runs on the game thread every
    // tick, which must not allocate.
    let high_water = players
        .array
        .header_field(guest, guest_layout::datum::layout::HIGH_WATER)
        .map_err(fault)?
        .clamp(0, 16);
    for slot in 0..high_water {
        let elem = players.array.element_addr(guest, slot).map_err(fault)?;
        let salt = guest.u16(elem)?;
        if salt == 0 {
            continue;
        }
        let handle = DatumHandle::new(salt, slot as u32);
        let player = players::Player::at(elem);
        if !player.listed(guest).map_err(fault)? {
            continue;
        }
        let slot = u32::from(handle.index());
        let stats = if has_stats {
            let row = engine + layout::ENGINE_PLAYER_STATS + slot * layout::PLAYER_STAT_SIZE;
            PlayerStats {
                score: game_score(guest, engine, row)?,
                kills: u32::from(guest.u16(row + layout::STAT_KILLS)?),
                deaths: u32::from(guest.u16(row + layout::STAT_DEATHS)?),
                assists: u32::from(guest.u16(row + layout::STAT_ASSISTS)?),
                suicides: u32::from(guest.u16(row + layout::STAT_SUICIDES)?),
            }
        } else {
            PlayerStats::ZERO
        };
        let local_user = player.local_user(guest).map_err(fault)?;
        let len = player.name_units(guest, &mut units).map_err(fault)?;
        // A bot's name as the record names it: without the mark the game
        // draws before it (docs/game/bots.md, "Clanker appearance and Live player options").
        let units = live_proto::bots::unmarked_units(&units[..len]);
        let name = previous
            .iter_mut()
            .find(|p| {
                p.datum == handle.raw() && p.player.name.encode_utf16().eq(units.iter().copied())
            })
            .map(|p| std::mem::take(&mut p.player.name))
            .unwrap_or_else(|| String::from_utf16_lossy(units));
        out.push(PlayerState {
            datum: handle.raw(),
            player: Player {
                index: slot,
                name,
                xuid: None,
                team: i32::from(player.team(guest).map_err(fault)?),
                local: Some(local_user != -1),
                appearance: None,
                bot: None,
            },
            appearance: appearance(guest, local_user)?,
            local_user: u8::try_from(local_user)
                .ok()
                .filter(|u| u32::from(*u) < layout::PLAYER_SLOT_COUNT),
            controller: if local_user == -1 {
                None
            } else {
                u8::try_from(guest.u32(elem + layout::PLAYER_CONTROLLER)? as i32)
                    .ok()
                    .filter(|c| *c < 4)
            },
            machine: player.machine(guest).map_err(fault)?,
            unit: guest_layout::handle::raw_or_none(player.unit(guest).map_err(fault)?),
            stats,
        });
    }
    Ok(())
}

pub fn team_scores(guest: &Guest, players: &[PlayerState]) -> ReadResult<Vec<TeamScore>> {
    let mut out = Vec::new();
    team_scores_into(guest, players, &mut out)?;
    Ok(out)
}

/// [`team_scores`] into `out` (cleared first): each team with a player on
/// it, in team order.
fn team_scores_into(
    guest: &Guest,
    players: &[PlayerState],
    out: &mut Vec<TeamScore>,
) -> ReadResult<()> {
    out.clear();
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    let statborg = engine + layout::ENGINE_STATBORG;
    if guest.u32(statborg)? == 0 {
        return Ok(());
    }
    let mut on = [false; 8];
    for team in players.iter().map(|p| p.player.team) {
        if let Some(on) = usize::try_from(team).ok().and_then(|t| on.get_mut(t)) {
            *on = true;
        }
    }
    for (team, _) in (0..).zip(on).filter(|(_, on)| *on) {
        let row = engine + layout::ENGINE_TEAM_STATS + team as u32 * layout::TEAM_STAT_SIZE;
        out.push(TeamScore {
            team,
            score: game_score(guest, engine, row)?,
        });
    }
    Ok(())
}

/// A statborg row's score for the whole game, its earlier rounds' included:
/// the round reset zeroes the round's (`guest_layout::game::game_score`).
fn game_score(guest: &Guest, engine: u32, row: u32) -> ReadResult<i64> {
    Ok(i64::from(guest_layout::game::game_score(
        i16(guest.u16(row + layout::STAT_SCORE)?),
        i16(guest.u16(row + layout::STAT_ROUNDS_SCORE)?),
        guest.u8(engine + layout::ENGINE_GAME_END_SEEN)? != 0,
    )))
}

/// Object lookups for one observation: the objects array's data pointer,
/// high-water mark and element size are read on the first lookup and kept
/// (the guest does not run while telemetry observes it), so each further
/// datum costs its own header's salt, type and body reads only.
pub(crate) struct Objects<'g> {
    guest: &'g Guest,
    table: std::cell::Cell<Option<Table>>,
}

#[derive(Clone, Copy)]
struct Table {
    data: u32,
    high_water: i32,
    element_size: i32,
}

impl<'g> Objects<'g> {
    pub(crate) fn new(guest: &'g Guest) -> Self {
        Self {
            guest,
            table: std::cell::Cell::new(None),
        }
    }

    fn table(&self) -> ReadResult<Table> {
        if let Some(table) = self.table.get() {
            return Ok(table);
        }
        // A null array or data pointer is an error, not "no object".
        let array = ptr(self.guest, layout::OBJECTS_PTR)?;
        let data = ptr(self.guest, array + layout::DATUM_DATA)?;
        let table = Table {
            data,
            high_water: self.guest.u32(array + layout::DATUM_HIGH_WATER)? as i32,
            element_size: self
                .guest
                .u32(array + guest_layout::datum::layout::ELEMENT_SIZE)? as i32,
        };
        self.table.set(Some(table));
        Ok(table)
    }

    /// An object's concrete type and body address, if `datum` names a live
    /// object: the high-water, salt and non-null body checks of
    /// `0x000badc0`, as `guest_layout::view::objects::get` makes them.
    fn body(&self, datum: u32) -> ReadResult<Option<(u8, u32)>> {
        let Some(handle) = DatumHandle::from_raw(datum) else {
            return Ok(None);
        };
        let table = self.table()?;
        let index = i32::from(handle.index());
        if index >= table.high_water {
            return Ok(None);
        }
        let header = (table.element_size.wrapping_mul(index) as u32).wrapping_add(table.data);
        let salt = self.guest.u16(header)?;
        if salt == 0 || salt != handle.salt() {
            return Ok(None);
        }
        let body = self.guest.u32(header + layout::OBJECT_BODY)?;
        if body == 0 {
            return Ok(None);
        }
        Ok(Some((self.guest.u8(header + layout::OBJECT_TYPE)?, body)))
    }
}

/// Three `f32`s in one guest read: the same bytes as three reads, and a
/// fault where any of them would fault.
fn vector(guest: &Guest, address: u32) -> ReadResult<[f64; 3]> {
    let b = guest.read::<12>(address)?;
    let f = |i: usize| f64::from(f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]));
    Ok([f(0), f(4), f(8)])
}

// The body's position, forward and up lie end to end.
const _: () = assert!(
    layout::BODY_FORWARD == layout::BODY_POSITION + 12
        && layout::BODY_UP == layout::BODY_POSITION + 24
);

/// A body's position, forward and up, in one read (it faults where any of
/// the three would).
fn pose(guest: &Guest, body: u32) -> ReadResult<[[f64; 3]; 3]> {
    let b = guest.read::<36>(body + layout::BODY_POSITION)?;
    let f = |i: usize| f64::from(f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]));
    Ok([
        [f(0), f(4), f(8)],
        [f(12), f(16), f(20)],
        [f(24), f(28), f(32)],
    ])
}

/// The unit's aiming vector, normalised, or `None` unless it is finite and
/// within 10% of unit length (the "likely" offset must not send garbage).
fn aim(guest: &Guest, body: u32) -> ReadResult<Option<[f64; 3]>> {
    Ok(normalised_aim(vector(guest, body + layout::UNIT_AIM)?))
}

fn normalised_aim(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    (v.iter().all(|c| c.is_finite()) && (0.9..=1.1).contains(&len))
        .then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// The weapon the unit holds: current weapon index `+0x212` into the four
/// weapon datums at `+0x218`. The weapon must be a live weapon object
/// parented to this unit (`0x00101fb0` finds a weapon's owner through its
/// parent).
/// With the weapon, its body (for the magazine).
fn held_weapon(objects: &Objects, unit: u32, body: u32) -> ReadResult<Option<(Weapon, u32)>> {
    let guest = objects.guest;
    let index = guest.u8(body + layout::UNIT_WEAPON_INDEX)? as i8;
    if !(0..layout::UNIT_WEAPON_SLOTS).contains(&index) {
        return Ok(None);
    }
    let datum = guest.u32(body + layout::UNIT_WEAPONS + 4 * index as u32)?;
    let Some((kind, weapon)) = objects.body(datum)? else {
        return Ok(None);
    };
    if kind != layout::OBJECT_TYPE_WEAPON || guest.u32(weapon + layout::BODY_PARENT)? != unit {
        return Ok(None);
    }
    let definition = guest.u32(weapon + layout::BODY_DEFINITION)?;
    Ok((definition != u32::MAX).then_some((Weapon { datum, definition }, weapon)))
}

/// Definition of any live object datum, including weapons and equipment.
pub fn object_definition(guest: &Guest, datum: u32) -> ReadResult<Option<u32>> {
    let Some((_, body)) = Objects::new(guest).body(datum)? else {
        return Ok(None);
    };
    Ok(Some(guest.u32(body + layout::BODY_DEFINITION)?).filter(|d| *d != u32::MAX))
}

pub fn object(guest: &Guest, datum: u32) -> ReadResult<Option<ObjectState>> {
    object_in(&Objects::new(guest), datum)
}

/// [`object`], its lookups through `objects`.
pub(crate) fn object_in(objects: &Objects, datum: u32) -> ReadResult<Option<ObjectState>> {
    object_with_motion(objects, datum, true)
}

fn object_with_motion(
    objects: &Objects,
    datum: u32,
    diagnostics: bool,
) -> ReadResult<Option<ObjectState>> {
    let guest = objects.guest;
    let Some((kind, body)) = objects.body(datum)? else {
        return Ok(None);
    };
    if kind > 1 {
        return Ok(None);
    }
    let definition = Some(guest.u32(body + layout::BODY_DEFINITION)?).filter(|d| *d != u32::MAX);
    // The frame-model fields only add detail: a fault while following them
    // (a stale weapon datum, a "likely" offset) drops the field, not the
    // snapshot.
    let aim = aim(guest, body).ok().flatten();
    let held = held_weapon(objects, datum, body).ok().flatten();
    let weapon = held.map(|(w, _)| w);
    let motion = motion(guest, kind, body, held.map(|(_, b)| b), diagnostics);
    let team = guest
        .u16(body + layout::UNIT_TEAM)
        .ok()
        .map(|t| t as i16)
        .filter(|t| *t >= 0);
    let [mut pos, mut forward, mut up] = pose(guest, body)?;
    let mut vehicle_definition = None;
    let mut mount = None;
    // Whose transform `pos` is: the unit's, else what it rides.
    let mut moving = body;
    let parent = guest.u32(body + layout::BODY_PARENT)?;
    let vehicle = match objects.body(parent)? {
        Some((layout::OBJECT_TYPE_VEHICLE, parent_body)) => {
            vehicle_definition =
                Some(guest.u32(parent_body + layout::BODY_DEFINITION)?).filter(|d| *d != u32::MAX);
            // A gunner rides a turret, itself parented to the vehicle it is
            // mounted on, its transform relative to that one: the mount's is
            // where the gunner is in the world.
            let mounted = vehicle_mount(objects, parent_body)?;
            let at = mounted.map_or(parent_body, |(_, b)| b);
            mount = mounted.map(|(m, _)| m);
            [pos, forward, up] = pose(guest, at)?;
            moving = at;
            Some(parent)
        }
        _ => None,
    };
    let velocity = vector(guest, moving + layout::BODY_VELOCITY)
        .ok()
        .filter(|v| v.iter().all(|c| c.is_finite()));
    let seat = guest
        .u16(body + layout::UNIT_SEAT)
        .ok()
        .map(|s| s as i16)
        .filter(|s| *s >= 0)
        .map(|s| s as u16);
    let vitality = vitality(guest, body).ok().flatten();
    if !pos.iter().chain(forward.iter()).all(|v| v.is_finite()) {
        return Ok(None);
    }
    Ok(Some(ObjectState {
        pos,
        forward,
        vehicle,
        mount,
        definition,
        aim,
        weapon,
        up,
        vehicle_definition,
        team,
        motion,
        velocity,
        seat,
        vitality,
    }))
}

/// Body and shield vitality of the object whose data is at `body`, as
/// `object_get_health` and `object_get_shield` read them: both 0 once the
/// body is depleted; `None` when either is not finite.
fn vitality(guest: &Guest, body: u32) -> ReadResult<Option<(f64, f64)>> {
    if guest.u16(body + layout::BODY_DAMAGE_FLAGS)? & layout::BODY_DEPLETED != 0 {
        return Ok(Some((0.0, 0.0)));
    }
    let health = f64::from(guest.f32(body + layout::BODY_VITALITY)?);
    let shield = f64::from(guest.f32(body + layout::BODY_SHIELD_VITALITY)?);
    Ok((health.is_finite() && shield.is_finite()).then_some((health, shield)))
}

/// The vehicle a vehicle (body at `body`) is mounted on, and that one's
/// body: a turret's parent, when the parent is a vehicle.
fn vehicle_mount(objects: &Objects, body: u32) -> ReadResult<Option<(u32, u32)>> {
    let parent = objects.guest.u32(body + layout::BODY_PARENT)?;
    Ok(match objects.body(parent)? {
        Some((layout::OBJECT_TYPE_VEHICLE, parent_body)) => Some((parent, parent_body)),
        _ => None,
    })
}

/// A unit's movement and inventory state; each field independently `None`
/// when its read faults or reads as nonsense.
/// `weapon` is the held weapon's body.
fn motion(guest: &Guest, kind: u8, body: u32, weapon: Option<u32>, diagnostics: bool) -> Motion {
    let velocity = vector(guest, body + layout::BODY_VELOCITY)
        .ok()
        .filter(|v| v.iter().all(|c| c.is_finite()))
        .unwrap_or_default();
    let crouch = guest
        .f32(body + layout::UNIT_CROUCH)
        .ok()
        .map(f64::from)
        .filter(|c| (0.0..=1.0).contains(c));
    let physics_mode = (kind == layout::OBJECT_TYPE_BIPED)
        .then(|| guest.u8(body + layout::BIPED_PHYSICS_MODE).ok())
        .flatten()
        .filter(|m| (1..=7).contains(m));
    let grenade = guest
        .u8(body + layout::UNIT_GRENADE_INDEX)
        .ok()
        .map(|g| g as i8)
        .filter(|g| (0..2).contains(g));
    let grenades = (|| -> ReadResult<Option<u32>> {
        let a = guest.u8(body + layout::UNIT_GRENADE_COUNTS)? as i8;
        let b = guest.u8(body + layout::UNIT_GRENADE_COUNTS + 1)? as i8;
        Ok((a >= 0 && b >= 0).then(|| a as u32 + b as u32))
    })()
    .ok()
    .flatten();
    let weapons = (|| -> ReadResult<[u32; 4]> {
        let mut slots = [u32::MAX; 4];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = guest.u32(body + layout::UNIT_WEAPONS + 4 * i as u32)?;
        }
        Ok(slots)
    })()
    .ok();
    let magazine = weapon.and_then(|weapon_body| {
        let reserve = guest
            .u16(weapon_body + layout::WEAPON_MAGAZINE_RESERVE)
            .ok()? as i16;
        let loaded = guest
            .u16(weapon_body + layout::WEAPON_MAGAZINE_LOADED)
            .ok()? as i16;
        Some((reserve, loaded))
    });
    let biped = kind == layout::OBJECT_TYPE_BIPED;
    let unit_flags = guest.u32(body + layout::UNIT_FLAGS).ok();
    let biped_flags = biped
        .then(|| guest.u16(body + layout::BIPED_FLAGS).ok())
        .flatten();
    let airborne_ticks = biped
        .then(|| guest.u8(body + layout::BIPED_AIRBORNE_TICKS).ok())
        .flatten()
        .map(|t| t as i8);
    let hitbox_offset = biped
        .then(|| guest.u8(body + layout::BIPED_HITBOX_OFFSET).ok())
        .flatten()
        .map(|t| t as i8);
    let havok = diagnostics
        .then(|| havok_body(guest, body).ok().flatten())
        .flatten();
    let support = (diagnostics && biped)
        .then(|| support(guest, body).ok())
        .flatten();
    Motion {
        velocity,
        crouch,
        physics_mode,
        grenade,
        grenades,
        weapons,
        magazine,
        animation: animation(guest, body).ok().flatten(),
        unit_flags,
        biped_flags,
        airborne_ticks,
        hitbox_offset,
        havok,
        support,
    }
}

/// A biped's ground support (`layout::BIPED_SUPPORT_*`) and the character
/// contacts on its physics component (`layout::CHARACTER_CONTACT_*`); the
/// contacts are empty without a component or with a count past
/// `layout::CHARACTER_CONTACTS_MAX`.
fn support(guest: &Guest, body: u32) -> ReadResult<Support> {
    let mut words = [0u32; 5];
    for (i, w) in words.iter_mut().enumerate() {
        *w = guest.u32(body + layout::BIPED_SUPPORT_WORDS + 4 * i as u32)?;
    }
    let mut contacts = Vec::new();
    let index = guest.u32(body + layout::BODY_PHYSICS)?;
    if index != u32::MAX {
        let array = guest.u32(layout::PHYSICS_COMPONENTS_PTR)?;
        let data = guest.u32(array + layout::DATUM_DATA)?;
        let component = data.wrapping_add((index & 0xffff) * layout::PHYSICS_COMPONENT_SIZE);
        let list = guest.u32(component + layout::COMPONENT_CHARACTER_CONTACTS)?;
        let count = guest.u32(component + layout::COMPONENT_CHARACTER_CONTACT_COUNT)?;
        if list != 0 && count <= layout::CHARACTER_CONTACTS_MAX {
            for i in 0..count {
                let c = list + i * layout::CHARACTER_CONTACT_SIZE;
                contacts.push(CharacterContact {
                    key: guest.u32(c + layout::CHARACTER_CONTACT_KEY)?,
                    other: guest.u32(c + layout::CHARACTER_CONTACT_OTHER)?,
                    point: vector(guest, c + layout::CHARACTER_CONTACT_POINT)?,
                    normal: vector(guest, c + layout::CHARACTER_CONTACT_NORMAL)?,
                    value: f64::from(guest.f32(c + layout::CHARACTER_CONTACT_VALUE)?),
                    flags: guest.u32(c + layout::CHARACTER_CONTACT_FLAGS)?,
                });
            }
        }
    }
    Ok(Support {
        flags: guest.u8(body + layout::BIPED_SUPPORT_FLAGS)?,
        words,
        point: vector(guest, body + layout::BIPED_SUPPORT_POINT)?,
        normal: vector(guest, body + layout::BIPED_SUPPORT_NORMAL)?,
        contact_point: vector(guest, body + layout::BIPED_SUPPORT_CONTACT_POINT)?,
        ground_state: vector(guest, body + layout::BIPED_GROUND_STATE)?,
        contacts,
    })
}

/// The first Havok rigid body's world position and linear velocity, as
/// `0x001d08e0` and `0x000dc67d` read them; `None` without a body or when
/// a value is not finite.
fn havok_body(guest: &Guest, body: u32) -> ReadResult<Option<([f64; 3], [f64; 3])>> {
    let index = guest.u32(body + layout::BODY_PHYSICS)?;
    if index == u32::MAX {
        return Ok(None);
    }
    let array = guest.u32(layout::PHYSICS_COMPONENTS_PTR)?;
    let data = guest.u32(array + layout::DATUM_DATA)?;
    let component = data.wrapping_add((index & 0xffff) * layout::PHYSICS_COMPONENT_SIZE);
    let bodies = guest.u32(component + layout::COMPONENT_BODIES)?;
    let rigid = guest.u32(bodies + layout::COMPONENT_BODY_RIGID)?;
    if rigid == 0 {
        return Ok(None);
    }
    let motion = guest.u32(rigid + layout::RIGID_MOTION)?;
    let pos = vector(guest, motion + layout::MOTION_POSITION)?;
    let vel = vector(guest, motion + layout::MOTION_VELOCITY)?;
    Ok(pos
        .iter()
        .chain(&vel)
        .all(|c| c.is_finite())
        .then_some((pos, vel)))
}

/// The animation the object whose data is at `body` plays: its animation
/// manager's first channel. `None` without a manager, a graph or an
/// animation, or when the frame reads as nonsense.
pub fn animation(guest: &Guest, body: u32) -> ReadResult<Option<Animation>> {
    Ok(channel_animation(guest, body)?.filter(|a| (0.0..32768.0).contains(&a.frame)))
}

// Distinguish a missing graph from an active channel containing a damaged sample.
fn channel_animation(guest: &Guest, body: u32) -> ReadResult<Option<Animation>> {
    let offset = guest.u16(body + layout::OBJECT_ANIMATION_MANAGER)? as i16;
    if !layout::MANAGER_OFFSETS.contains(&offset) {
        return Ok(None);
    }
    let channel = body + offset as u32 + layout::MANAGER_CHANNEL;
    let graph = guest.u32(channel + layout::CHANNEL_GRAPH)?;
    let inherited = guest.u16(channel + layout::CHANNEL_ANIMATION)? as i16;
    let index = guest.u16(channel + layout::CHANNEL_ANIMATION + 2)? as i16;
    let frame = guest.f32(channel + layout::CHANNEL_FRAME)?;
    Ok((graph != u32::MAX
        && index >= 0
        && inherited >= -1)
    .then_some(Animation {
        graph,
        inherited,
        index,
        frame,
    }))
}

/// Normalized travel for a machine with an active position animation.
fn machine_position(guest: &Guest, body: u32) -> ReadResult<Option<f64>> {
    let channel = body + layout::DEVICE_POSITION_CHANNEL;
    let graph = guest.u32(channel + layout::CHANNEL_GRAPH)?;
    let index = guest.u16(channel + layout::CHANNEL_ANIMATION + 2)? as i16;
    if graph == u32::MAX || index < 0 {
        return Ok(None);
    }
    let position = f64::from(guest.f32(body + layout::DEVICE_POSITION)?);
    Ok((0.0..=1.0).contains(&position).then_some(position))
}

/// Every live AI-controlled biped: object type biped, an actor datum at
/// `+0x12c`, no player at `+0x13c`, and not body-depleted.
pub fn actors(guest: &Guest) -> ReadResult<Vec<ActorState>> {
    Ok(world(guest)?.actors)
}

/// One pass over the object header table (12 bytes a slot up to the
/// high-water mark, at most [`layout::OBJECT_CAPACITY`]): AI bipeds, every
/// vehicle (occupied when any biped's parent is it) and every machine. Only
/// those objects' bodies are read; a fault on one object skips it.
pub fn world(guest: &Guest) -> ReadResult<World> {
    let mut out = World::default();
    world_into(guest, &mut ScanBuffers::default(), &mut out)?;
    Ok(out)
}

/// The world scan's working lists, kept between ticks for their allocations.
#[derive(Default)]
pub struct ScanBuffers {
    headers: Vec<u8>,
    /// Vehicles a biped is parented to.
    ridden: Vec<u32>,
}

/// [`world`] into `out`, reusing its lists' (and `buffers`') allocations
/// from the last tick: once they have grown, the scan allocates nothing.
/// After an error `out` is empty.
pub fn world_into(guest: &Guest, buffers: &mut ScanBuffers, out: &mut World) -> ReadResult<()> {
    clear(out);
    let scanned = scan(guest, buffers, out);
    if scanned.is_err() {
        clear(out);
    }
    scanned
}

fn clear(world: &mut World) {
    world.actors.clear();
    world.vehicles.clear();
    world.machines.clear();
    world.placed.clear();
    world.bodies.clear();
    world.projectiles.clear();
    world.items.clear();
}

/// Fetch the allocated object headers once while the simulation is stopped.
fn object_headers(guest: &Guest, headers: &mut Vec<u8>) -> ReadResult<()> {
    let array = ptr(guest, layout::OBJECTS_PTR)?;
    let field = |offset| array.checked_add(offset)
        .ok_or_else(|| fault(guest_layout::MemFault { addr: array, len: 4, write: false }));
    let high_water = guest.u32(field(layout::DATUM_HIGH_WATER)?)?;
    if high_water > layout::OBJECT_CAPACITY {
        return Err(fault(guest_layout::MemFault {addr: array, len: 4, write: false}));
    }
    let data = ptr(guest, field(layout::DATUM_DATA)?)?;
    headers.resize((high_water * layout::OBJECT_HEADER_SIZE) as usize, 0);
    guest.read_bytes(data, headers, 1)
}

/// Allocated salted datum, header type and body pointer in slot order.
/// A missing body in an allocated slot is distinguishable from a free slot.
fn object_rows(headers: &[u8]) -> impl Iterator<Item = (u32, u8, u32)> + '_ {
    const HEADER: usize = layout::OBJECT_HEADER_SIZE as usize;
    headers
        .as_chunks::<HEADER>()
        .0
        .iter()
        .enumerate()
        .filter_map(|(slot, header)| {
            let salt = u16::from_le_bytes([header[0], header[1]]);
            let at = layout::OBJECT_BODY as usize;
            let body = u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
            let datum = (u32::from(salt) << 16) | slot as u32;
            (salt != 0).then_some((datum, header[layout::OBJECT_TYPE as usize], body))
        })
}

fn scan(guest: &Guest, buffers: &mut ScanBuffers, out: &mut World) -> ReadResult<()> {
    let ScanBuffers { headers, ridden } = buffers;
    let objects = Objects::new(guest);
    object_headers(guest, headers)?;
    ridden.clear();
    for (datum, kind, body) in object_rows(headers).filter(|&(_, _, body)| body != 0) {
        // Not from a placement or a spawn, and no parent: loose.
        let mut loose = false;
        if let Some(kind) = placed_kind(kind) {
            // A fault skips the object, as below.
            loose = placed(guest, kind, body, &mut out.placed).unwrap_or(false);
        }
        match kind {
            layout::OBJECT_TYPE_BIPED => {
                if let Ok(parent) = guest.u32(body + layout::BODY_PARENT)
                    && parent != u32::MAX
                {
                    ridden.push(parent);
                }
                let Ok(depleted) = guest
                    .u16(body + layout::BODY_DAMAGE_FLAGS)
                    .map(|f| f & layout::BODY_DEPLETED != 0)
                else {
                    continue;
                };
                // A dead biped stays in the world as a body until the game
                // clears it away; whose it was is the session's business.
                if depleted {
                    if let Ok(Some(object)) = object_in(&objects, datum) {
                        out.bodies.push(ActorState { datum, object });
                    }
                    continue;
                }
                let ai = || -> ReadResult<bool> {
                    Ok(guest.u32(body + layout::UNIT_ACTOR)? != u32::MAX
                        && guest.u32(body + layout::UNIT_PLAYER)? == u32::MAX)
                };
                if !ai().unwrap_or(false) {
                    continue;
                }
                if let Ok(Some(object)) = object_in(&objects, datum) {
                    out.actors.push(ActorState { datum, object });
                }
            }
            layout::OBJECT_TYPE_VEHICLE => {
                if let Ok((definition, [pos, forward, up])) = transform(guest, body) {
                    // Turret detail only adds: a fault drops it, not the vehicle.
                    let parent = vehicle_mount(&objects, body).ok().flatten().map(|(p, _)| p);
                    out.vehicles.push(VehicleState {
                        datum,
                        definition,
                        pos,
                        forward,
                        up,
                        occupied: false,
                        parent,
                        aim: aim(guest, body).ok().flatten(),
                        velocity: finite_velocity(guest, body),
                    });
                }
            }
            layout::OBJECT_TYPE_PROJECTILE => {
                if let Ok(Some(p)) = projectile(&objects, datum, body) {
                    out.projectiles.push(p);
                }
            }
            layout::OBJECT_TYPE_WEAPON | layout::OBJECT_TYPE_EQUIPMENT if loose => {
                if let Ok(Some(i)) = item(guest, datum, kind, body) {
                    out.items.push(i);
                }
            }
            layout::OBJECT_TYPE_MACHINE => {
                let Ok((definition, [pos, forward, up])) = transform(guest, body) else {
                    continue;
                };
                let placement = guest
                    .u16(body + layout::BODY_PLACEMENT_INDEX)
                    .ok()
                    .filter(|p| (*p as i16) >= 0);
                let position = placement.and_then(|_| machine_position(guest, body).ok().flatten());
                out.machines.push(MachineState {
                    datum,
                    definition,
                    placement,
                    pos,
                    forward,
                    up,
                    position,
                });
            }
            _ => {}
        }
    }
    ridden.sort_unstable();
    for v in &mut out.vehicles {
        v.occupied = ridden.binary_search(&v.datum).is_ok();
    }
    Ok(())
}

/// Body `+0x88` when it reads and is finite.
fn finite_velocity(guest: &Guest, body: u32) -> Option<[f64; 3]> {
    vector(guest, body + layout::BODY_VELOCITY)
        .ok()
        .filter(|v| v.iter().all(|c| c.is_finite()))
}

/// A projectile: its definition, position and velocity; a stuck one (with
/// a parent) at its parent's position, its own being parent-relative.
fn projectile(objects: &Objects, datum: u32, body: u32) -> ReadResult<Option<ProjectileState>> {
    let guest = objects.guest;
    let (definition, [mut pos, _, _]) = transform(guest, body)?;
    let parent = Some(guest.u32(body + layout::BODY_PARENT)?).filter(|p| *p != u32::MAX);
    let mut velocity = finite_velocity(guest, body);
    if let Some(p) = parent {
        let Some((_, parent_body)) = objects.body(p)? else {
            return Ok(None);
        };
        pos = vector(guest, parent_body + layout::BODY_POSITION)?;
        velocity = None;
    }
    if !pos.iter().all(|c| c.is_finite()) {
        return Ok(None);
    }
    Ok(Some(ProjectileState {
        datum,
        definition,
        pos,
        velocity,
        parent,
    }))
}

/// A weapon or equipment object lying loose ([`placed`] found it has no
/// parent and neither a scenario placement nor a netgame spawn made it).
fn item(guest: &Guest, datum: u32, kind: u8, body: u32) -> ReadResult<Option<ItemState>> {
    let (definition, [pos, forward, up]) = transform(guest, body)?;
    if !pos.iter().chain(&forward).chain(&up).all(|c| c.is_finite()) {
        return Ok(None);
    }
    Ok(Some(ItemState {
        datum,
        definition,
        equipment: kind == layout::OBJECT_TYPE_EQUIPMENT,
        pos,
        forward,
        up,
    }))
}

/// The placement kind of an object type whose placements frames track.
fn placed_kind(object_type: u8) -> Option<ObjectKind> {
    PLACED_KINDS
        .get(usize::from(object_type))
        .copied()
        .flatten()
}

/// [`placed_kind`] by object type, made once.
static PLACED_KINDS: std::sync::LazyLock<[Option<ObjectKind>; 16]> =
    std::sync::LazyLock::new(|| {
        std::array::from_fn(|t| {
            let t = t as u8;
            layout::PLACEMENT_BLOCKS
                .iter()
                .any(|(block, _, _)| *block == t)
                .then(|| object_kind(t))
                .flatten()
        })
    });

/// The scenario block an object type is placed by (the type table's order).
pub fn object_kind(object_type: u8) -> Option<ObjectKind> {
    Some(match object_type {
        0 => ObjectKind::Biped,
        1 => ObjectKind::Vehicle,
        2 => ObjectKind::Weapon,
        3 => ObjectKind::Equipment,
        6 => ObjectKind::Scenery,
        7 => ObjectKind::Machine,
        8 => ObjectKind::Control,
        9 => ObjectKind::LightFixture,
        10 => ObjectKind::SoundScenery,
        11 => ObjectKind::Crate,
        12 => ObjectKind::Creature,
        _ => return None,
    })
}

/// Any live object, as [`things`] lists it: its object type (the header's
/// `+3`: 2 weapon, 3 equipment, 6 scenery, 8 control, 11 crate and so on,
/// [`object_kind`]'s table), definition and pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThingState {
    pub datum: u32,
    pub object_type: u8,
    pub definition: Option<u32>,
    /// The object it is attached to (a held weapon's unit); its pose is
    /// then relative to that object.
    pub parent: Option<u32>,
    /// The scenario placement it came from (`+0x1a`).
    pub placement: Option<u16>,
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
}

/// Every live object whose type `wanted` takes, in slot order: one pass
/// over the object header table, as [`world`] makes, reading only the
/// wanted objects' bodies. A fault on one object skips it. For the agent
/// API's object kinds the telemetry scan does not follow (scenery, items
/// on the ground, device controls, crates).
pub fn things(guest: &Guest, wanted: impl Fn(u8) -> bool) -> ReadResult<Vec<ThingState>> {
    let mut headers = Vec::new();
    object_headers(guest, &mut headers)?;
    let mut out = Vec::new();
    for (datum, object_type, body) in
        object_rows(&headers).filter(|&(_, kind, body)| body != 0 && wanted(kind))
    {
        let read = || -> ReadResult<ThingState> {
            let (definition, [pos, forward, up]) = transform(guest, body)?;
            let parent = Some(guest.u32(body + layout::BODY_PARENT)?).filter(|p| *p != u32::MAX);
            let placement =
                Some(guest.u16(body + layout::BODY_PLACEMENT_INDEX)?).filter(|p| (*p as i16) >= 0);
            Ok(ThingState {
                datum,
                object_type,
                definition,
                parent,
                placement,
                pos,
                forward,
                up,
            })
        };
        if let Ok(thing) = read() {
            out.push(thing);
        }
    }
    Ok(out)
}

/// Notes an object that came from a scenario placement (`+0x1a`) or a
/// netgame equipment entry (`+0xaf`). True for one that came from neither
/// and has no parent: lying loose (a dropped weapon), from the same reads.
fn placed(
    guest: &Guest,
    kind: ObjectKind,
    body: u32,
    out: &mut Vec<PlacedObject>,
) -> ReadResult<bool> {
    // The parent and the placement index in one read.
    const PARENT: usize = 0;
    const INDEX: usize = (layout::BODY_PLACEMENT_INDEX - layout::BODY_PARENT) as usize;
    let b = guest.read::<{ INDEX + 2 }>(body + layout::BODY_PARENT)?;
    let index = i16::from_le_bytes([b[INDEX], b[INDEX + 1]]);
    let netgame = guest.u8(body + layout::BODY_NETGAME_EQUIPMENT)? as i8;
    let parented = u32::from_le_bytes(b[PARENT..PARENT + 4].try_into().unwrap()) != u32::MAX;
    if index < 0 && netgame < 0 {
        return Ok(!parented);
    }
    let [pos, forward, up] = pose(guest, body)?;
    let mut push = |kind, index| {
        out.push(PlacedObject {
            kind,
            index,
            parented,
            pos,
            forward,
            up,
        })
    };
    if index >= 0 {
        push(kind, index as u16);
    }
    if netgame >= 0 {
        push(ObjectKind::NetgameEquipment, netgame as u16);
    }
    Ok(false)
}

/// Forward and up of a scenario rotation (yaw, pitch, roll in radians):
/// `Rz(yaw) · Ry(−pitch) · Rx(roll)` applied to +x and +z, as map.glb's
/// placements are built (docs/live/live-art.md).
pub fn rotation_axes([yaw, pitch, roll]: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let rot = |v: [f64; 3]| {
        let v = [v[0], cr * v[1] - sr * v[2], sr * v[1] + cr * v[2]];
        let v = [cp * v[0] - sp * v[2], v[1], sp * v[0] + cp * v[2]];
        [cy * v[0] - sy * v[1], sy * v[0] + cy * v[1], v[2]]
    };
    (rot([1.0, 0.0, 0.0]), rot([0.0, 0.0, 1.0]))
}

/// One placement block's poses: `(count, address)` at `at`, records of
/// `stride` bytes with the position at `position` and the rotation at
/// `rotation`.
fn block_poses(
    guest: &Guest,
    at: u32,
    stride: u32,
    position: u32,
    rotation: u32,
) -> ReadResult<Vec<Option<Pose>>> {
    let count = guest.u32(at)?;
    let address = guest.u32(at + 4)?;
    if count == 0 || address == 0 || count > layout::MAX_PLACEMENTS {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0; (count * stride) as usize];
    guest.read_bytes(address, &mut bytes, 1)?;
    let f = |o: usize| f64::from(f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()));
    Ok((0..count as usize)
        .map(|i| {
            let r = i * stride as usize;
            let v = |at: u32| [0, 4, 8].map(|k| f(r + at as usize + k));
            let (pos, rot) = (v(position), v(rotation));
            if !pos.iter().chain(&rot).all(|c| c.is_finite()) {
                return None;
            }
            let (forward, up) = rotation_axes(rot);
            Some(Pose { pos, forward, up })
        })
        .collect())
}

/// The loaded scenario's placements for every tracked kind and its netgame
/// equipment (read once a game: the scenario does not change under it). A
/// block that faults is left empty.
pub fn placements(guest: &Guest) -> ReadResult<Placements> {
    let scenario = ptr(guest, layout::SCENARIO_PTR)?;
    let mut kinds: Vec<(ObjectKind, Vec<Option<Pose>>)> = layout::PLACEMENT_BLOCKS
        .iter()
        .filter_map(|&(object_type, offset, stride)| {
            let poses = block_poses(
                guest,
                scenario + offset,
                stride,
                layout::PLACEMENT_POSITION,
                layout::PLACEMENT_ROTATION,
            );
            Some((object_kind(object_type)?, poses.unwrap_or_default()))
        })
        .collect();
    let netgame = block_poses(
        guest,
        scenario + layout::NETGAME_EQUIPMENT,
        layout::NETGAME_EQUIPMENT_STRIDE,
        layout::NETGAME_EQUIPMENT_POSITION,
        layout::NETGAME_EQUIPMENT_ROTATION,
    );
    kinds.push((ObjectKind::NetgameEquipment, netgame.unwrap_or_default()));
    kinds.sort_by_key(|(k, _)| *k);
    Ok(Placements { kinds })
}

/// An object's definition and its own transform (parent-relative when it
/// has a parent).
fn transform(guest: &Guest, body: u32) -> ReadResult<(Option<u32>, [[f64; 3]; 3])> {
    let definition = Some(guest.u32(body + layout::BODY_DEFINITION)?).filter(|d| *d != u32::MAX);
    Ok((definition, pose(guest, body)?))
}

/// The active structure BSP index, `None` when negative.
pub fn active_bsp(guest: &Guest) -> ReadResult<Option<u32>> {
    let globals = ptr(guest, layout::GAME_GLOBALS_PTR)?;
    let bsp = guest.u16(globals + layout::GLOBALS_ACTIVE_BSP)? as i16;
    Ok(u32::try_from(bsp).ok())
}

/// Whether a HaloScript cinematic is playing (between `cinematic_start`
/// and `cinematic_stop`): the cinematic block's in-progress byte, false
/// with no block, as the game's own `0x0013cb40` reads it.
pub fn cinematic(guest: &Guest) -> ReadResult<bool> {
    let block = guest.u32(layout::CINEMATIC_GLOBALS_PTR)?;
    if block == 0 {
        return Ok(false);
    }
    Ok(guest.u8(block + layout::CINEMATIC_IN_PROGRESS)? != 0)
}

/// What the damage hook records: the report at `report` for `victim`, with
/// the victim's position (its own `+0x64`) when it is a live object.
pub fn hit(guest: &Guest, report: u32, victim: u32) -> ReadResult<HitRecord> {
    let pos = match Objects::new(guest).body(victim)? {
        Some((_, body)) => Some(vector(guest, body + layout::BODY_POSITION)?)
            .filter(|p| p.iter().all(|c| c.is_finite())),
        None => None,
    };
    Ok(HitRecord {
        code: guest.u8(report + layout::REPORT_CODE)?,
        effect: guest.u32(report + layout::REPORT_EFFECT)?,
        owner_player: guest.u32(report + layout::REPORT_OWNER_PLAYER)?,
        owner_object: guest.u32(report + layout::REPORT_OWNER_OBJECT)?,
        victim,
        pos,
        body: guest.f32(report + layout::REPORT_BODY)?,
        shield: guest.f32(report + layout::REPORT_SHIELD)?,
        tick: game_tick(guest).ok(),
    })
}

pub fn snapshot(guest: &Guest) -> ReadResult<Snapshot> {
    snapshot_after(guest, &mut [])
}

/// [`snapshot`], its roster read by [`players_after`] (names from
/// `previous` while they are unchanged).
pub fn snapshot_after(guest: &Guest, previous: &mut [PlayerState]) -> ReadResult<Snapshot> {
    snapshot_with_motion(guest, previous, true)
}

/// The sim observer reads the player pose, inventory and stats each tick.
/// The live inspector alone consumes the optional Havok body and contact lists.
pub fn sim_snapshot(guest: &Guest) -> ReadResult<Snapshot> {
    snapshot_with_motion(guest, &mut [], false)
}

fn snapshot_with_motion(
    guest: &Guest,
    previous: &mut [PlayerState],
    diagnostics: bool,
) -> ReadResult<Snapshot> {
    let players = players_after(guest, previous)?;
    let teams = team_scores(guest, &players)?;
    let lookups = Objects::new(guest);
    let objects = players
        .iter()
        .map(|p| object_with_motion(&lookups, p.unit, diagnostics))
        .collect::<ReadResult<Vec<_>>>()?;
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    Ok(Snapshot {
        tick: game_tick(guest)?,
        has_stats: guest.u32(engine + layout::ENGINE_STATBORG)? != 0,
        players,
        teams,
        objects,
        shots: Default::default(),
        actors: Vec::new(),
        vehicles: Vec::new(),
        machines: Vec::new(),
        placed: Vec::new(),
        bodies: Vec::new(),
        projectiles: Vec::new(),
        items: Vec::new(),
        bsp: active_bsp(guest).ok().flatten(),
        hits: Vec::new(),
        cinematic: cinematic(guest).unwrap_or(false),
    })
}

/// [`snapshot`] into `out`, reusing its lists' allocations (the world
/// lists, shots and hits are left as they are). Names come from `out`'s
/// last roster while they are unchanged ([`players_after`]); `spare` is a
/// second roster list, swapped with it. After an error `out` is partly
/// filled.
pub fn snapshot_into(
    guest: &Guest,
    out: &mut Snapshot,
    spare: &mut Vec<PlayerState>,
) -> ReadResult<()> {
    let mut previous = std::mem::replace(&mut out.players, std::mem::take(spare));
    let read = players_into(guest, &mut previous, &mut out.players);
    previous.clear();
    *spare = previous;
    read?;
    team_scores_into(guest, &out.players, &mut out.teams)?;
    let lookups = Objects::new(guest);
    out.objects.clear();
    for p in &out.players {
        out.objects.push(object_in(&lookups, p.unit)?);
    }
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    out.tick = game_tick(guest)?;
    out.has_stats = guest.u32(engine + layout::ENGINE_STATBORG)? != 0;
    out.bsp = active_bsp(guest).ok().flatten();
    out.cinematic = cinematic(guest).unwrap_or(false);
    Ok(())
}

/// The scenario tag datum of the loaded map's tag index, to check a
/// host-side name table against (`tags::TagNames::is_for`).
pub fn tag_index_scenario(guest: &Guest) -> ReadResult<u32> {
    let index = ptr(guest, layout::TAG_INDEX_PTR)?;
    guest.u32(index + layout::TAG_INDEX_SCENARIO)
}

pub fn yaw(forward: [f64; 3]) -> f64 {
    forward[1].atan2(forward[0])
}

#[cfg(test)]
pub(crate) mod fixture {
    use runtime::{Guest, guest::Permissions};

    pub const GLOBALS: u32 = 0x100000;
    pub const TIME: u32 = 0x110000;
    pub const ENGINE: u32 = 0x120000;
    pub const PLAYERS: u32 = 0x130000;
    pub const PLAYER_DATA: u32 = 0x131000;
    pub const OBJECTS: u32 = 0x140000;
    pub const OBJECT_DATA: u32 = 0x141000;
    pub const BODY: u32 = 0x150000;
    /// The cinematic block (only where a test points
    /// [`super::layout::CINEMATIC_GLOBALS_PTR`] at it).
    pub const CINEMATIC: u32 = 0x118000;

    /// Points the game at a cinematic block and sets its in-progress byte
    /// (`playing`) or clears it.
    pub fn set_cinematic(guest: &mut Guest, playing: bool) {
        guest
            .set_u32(super::layout::CINEMATIC_GLOBALS_PTR, CINEMATIC)
            .unwrap();
        guest
            .set_u8(
                CINEMATIC + super::layout::CINEMATIC_IN_PROGRESS,
                u8::from(playing),
            )
            .unwrap();
    }
    /// Bodies of slots past [`LOW_BODIES`] (only in [`large_memory`]).
    pub const HIGH_BODY: u32 = 0x0100_0000;
    /// Object slots whose bodies fit below the 0x200000 end of [`memory`].
    pub const LOW_BODIES: u32 = (0x200000 - BODY) / 0x1000;
    /// Room for scenario placement blocks (only in [`large_memory`]).
    pub const SCENARIO: u32 = 0x0180_0000;

    /// An object slot's body address: 4 KiB a slot.
    pub fn body(slot: u32) -> u32 {
        if slot < LOW_BODIES {
            BODY + slot * 0x1000
        } else {
            HIGH_BODY + slot * 0x1000
        }
    }

    /// [`memory`] with room for a full object table's bodies (every slot up
    /// to [`super::layout::OBJECT_CAPACITY`]) and a scenario at [`SCENARIO`].
    pub fn large_memory() -> Guest {
        let mut guest = memory();
        guest
            .map_zeroed(HIGH_BODY, 0x0100_0000, Permissions::RW)
            .unwrap();
        guest
    }

    pub fn memory() -> Guest {
        let mut guest = Guest::new();
        guest
            .map_zeroed(0x100000, 0x100000, Permissions::RW)
            .unwrap();
        guest
            .map_zeroed(0x400000, 0x200000, Permissions::RW)
            .unwrap();
        guest
            .set_u32(super::layout::GAME_GLOBALS_PTR, GLOBALS)
            .unwrap();
        guest.set_u32(super::layout::GAME_TIME_PTR, TIME).unwrap();
        guest
            .set_u32(super::layout::ENGINE_GLOBALS_PTR, ENGINE)
            .unwrap();
        guest.set_u32(super::layout::PLAYERS_PTR, PLAYERS).unwrap();
        guest.set_u32(super::layout::OBJECTS_PTR, OBJECTS).unwrap();
        guest
            .set_u32(PLAYERS + super::layout::DATUM_DATA, PLAYER_DATA)
            .unwrap();
        guest
            .set_u32(OBJECTS + super::layout::DATUM_DATA, OBJECT_DATA)
            .unwrap();
        // The views read the element size from the array, as the game does.
        guest
            .set_u32(
                PLAYERS + guest_layout::datum::layout::ELEMENT_SIZE,
                super::layout::PLAYER_SIZE,
            )
            .unwrap();
        guest
            .set_u32(
                OBJECTS + guest_layout::datum::layout::ELEMENT_SIZE,
                super::layout::OBJECT_HEADER_SIZE,
            )
            .unwrap();
        guest
            .set_u32(ENGINE + super::layout::ENGINE_STATBORG, 1)
            .unwrap();
        guest
    }

    pub fn put_player(guest: &mut Guest, slot: u32, name: &str, team: i8, local: bool, unit: u32) {
        let address = PLAYER_DATA + slot * super::layout::PLAYER_SIZE;
        guest.set_u16(address, 0x8001 + slot as u16).unwrap();
        guest
            .set_u16(
                address + super::layout::PLAYER_LOCAL_USER,
                if local { 0 } else { u16::MAX },
            )
            .unwrap();
        guest
            .set_u32(address + super::layout::PLAYER_UNIT, unit)
            .unwrap();
        guest
            .set_u8(address + super::layout::PLAYER_TEAM, team as u8)
            .unwrap();
        for (i, c) in name.encode_utf16().enumerate() {
            guest
                .set_u16(address + super::layout::PLAYER_NAME + 2 * i as u32, c)
                .unwrap();
        }
        guest
            .set_u32(PLAYERS + super::layout::DATUM_HIGH_WATER, (slot + 1).max(1))
            .unwrap();
        let row =
            ENGINE + super::layout::ENGINE_PLAYER_STATS + slot * super::layout::PLAYER_STAT_SIZE;
        guest
            .set_u16(row + super::layout::STAT_SCORE, 10 + slot as u16)
            .unwrap();
        guest
            .set_u16(row + super::layout::STAT_KILLS, 1 + slot as u16)
            .unwrap();
        guest
            .set_u16(row + super::layout::STAT_DEATHS, 2 + slot as u16)
            .unwrap();
        guest
            .set_u16(row + super::layout::STAT_ASSISTS, 3 + slot as u16)
            .unwrap();
        guest
            .set_u16(row + super::layout::STAT_SUICIDES, 4 + slot as u16)
            .unwrap();
    }

    /// Signs a local player slot in and writes its profile appearance block:
    /// the four palette colours and the two emblem layers.
    pub fn put_slot_profile(
        guest: &mut Guest,
        slot: u32,
        colors: [u8; 4],
        emblem_foreground: u8,
        emblem_background: u8,
    ) {
        let address = super::layout::PLAYER_SLOTS + slot * super::layout::PLAYER_SLOT_SIZE;
        guest
            .set_u32(
                address + super::layout::PLAYER_SLOT_FLAGS,
                super::layout::PLAYER_SLOT_SIGNED_IN,
            )
            .unwrap();
        let block =
            address + super::layout::PLAYER_SLOT_PROFILE + super::layout::PROFILE_APPEARANCE;
        for (i, color) in colors.into_iter().enumerate() {
            guest.set_u8(block + i as u32, color).unwrap();
        }
        guest
            .set_u8(
                block + super::layout::APPEARANCE_EMBLEM_FOREGROUND,
                emblem_foreground,
            )
            .unwrap();
        guest
            .set_u8(
                block + super::layout::APPEARANCE_EMBLEM_BACKGROUND,
                emblem_background,
            )
            .unwrap();
    }

    pub fn put_object(guest: &mut Guest, slot: u32, parent: u32) {
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest.set_u16(header, 0x9001 + slot as u16).unwrap();
        guest
            .set_u8(header + super::layout::OBJECT_TYPE, 0)
            .unwrap();
        guest
            .set_u32(header + super::layout::OBJECT_BODY, body(slot))
            .unwrap();
        let body = body(slot);
        guest
            .set_u32(body + super::layout::BODY_PARENT, parent)
            .unwrap();
        for (i, value) in [1.0f32, 2.0, 3.0].into_iter().enumerate() {
            guest
                .set_f32(body + super::layout::BODY_POSITION + 4 * i as u32, value)
                .unwrap();
        }
        guest
            .set_f32(body + super::layout::BODY_FORWARD, 0.0)
            .unwrap();
        guest
            .set_f32(body + super::layout::BODY_FORWARD + 4, 1.0)
            .unwrap();
        guest
            .set_f32(body + super::layout::BODY_FORWARD + 8, 0.0)
            .unwrap();
        guest
            .set_f32(body + super::layout::BODY_UP + 8, 1.0)
            .unwrap();
        guest
            .set_u32(body + super::layout::UNIT_ACTOR, u32::MAX)
            .unwrap();
        guest
            .set_u32(body + super::layout::UNIT_PLAYER, u32::MAX)
            .unwrap();
        // Not from a placement or a netgame spawn, as `object_new` leaves
        // dynamic objects.
        guest
            .set_u16(body + super::layout::BODY_PLACEMENT_INDEX, u16::MAX)
            .unwrap();
        guest
            .set_u8(body + super::layout::BODY_NETGAME_EQUIPMENT, 0xff)
            .unwrap();
        let high_water = OBJECTS + super::layout::DATUM_HIGH_WATER;
        let current = guest.u32(high_water).unwrap();
        guest.set_u32(high_water, current.max(slot + 1)).unwrap();
    }

    /// An AI biped at `slot`: an actor datum, team, and position `pos`.
    pub fn put_actor(guest: &mut Guest, slot: u32, team: i16, pos: [f32; 3]) {
        put_object(guest, slot, u32::MAX);
        let body = body(slot);
        guest
            .set_u32(body + super::layout::UNIT_ACTOR, 0xa000_0000 | slot)
            .unwrap();
        guest
            .set_u16(body + super::layout::UNIT_TEAM, team as u16)
            .unwrap();
        for (i, value) in pos.into_iter().enumerate() {
            guest
                .set_f32(body + super::layout::BODY_POSITION + 4 * i as u32, value)
                .unwrap();
        }
    }

    pub fn put_vehicle(guest: &mut Guest, slot: u32) {
        put_object(guest, slot, u32::MAX);
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest
            .set_u8(header + super::layout::OBJECT_TYPE, 1)
            .unwrap();
        let body = body(slot);
        for (i, value) in [10.0f32, 20.0, 30.0].into_iter().enumerate() {
            guest
                .set_f32(body + super::layout::BODY_POSITION + 4 * i as u32, value)
                .unwrap();
        }
        guest
            .set_f32(body + super::layout::BODY_FORWARD, -1.0)
            .unwrap();
        guest
            .set_f32(body + super::layout::BODY_FORWARD + 4, 0.0)
            .unwrap();
    }

    /// A machine at `slot`: placed by the scenario at `placement` (or not,
    /// with -1), device position `position`.
    pub fn put_machine(guest: &mut Guest, slot: u32, placement: i16, position: f32) {
        put_object(guest, slot, u32::MAX);
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest
            .set_u8(
                header + super::layout::OBJECT_TYPE,
                super::layout::OBJECT_TYPE_MACHINE,
            )
            .unwrap();
        let body = body(slot);
        guest
            .set_u16(body + super::layout::BODY_PLACEMENT_INDEX, placement as u16)
            .unwrap();
        guest.set_u32(body + 0x18c, 0xe100_0010).unwrap();
        guest.set_u16(body + 0x190, u16::MAX).unwrap();
        guest.set_u16(body + 0x192, 2).unwrap();
        guest.set_f32(body + 0x140, position).unwrap();
    }

    /// An object of `object_type` at `slot` from placement `placement`
    /// (-1 for none) and netgame equipment entry `netgame` (-1 for none),
    /// at `pos`, held by `parent`.
    pub fn put_placed(
        guest: &mut Guest,
        slot: u32,
        object_type: u8,
        placement: i16,
        netgame: i8,
        pos: [f32; 3],
        parent: u32,
    ) {
        put_object(guest, slot, parent);
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest
            .set_u8(header + super::layout::OBJECT_TYPE, object_type)
            .unwrap();
        let body = body(slot);
        guest
            .set_u16(body + super::layout::BODY_PLACEMENT_INDEX, placement as u16)
            .unwrap();
        guest
            .set_u8(body + super::layout::BODY_NETGAME_EQUIPMENT, netgame as u8)
            .unwrap();
        for (i, value) in pos.into_iter().enumerate() {
            guest
                .set_f32(body + super::layout::BODY_POSITION + 4 * i as u32, value)
                .unwrap();
        }
    }

    pub fn object_datum(slot: u32) -> u32 {
        ((0x9001 + slot) << 16) | slot
    }

    /// Gives the unit object at `slot` a definition, an aiming vector and
    /// a current weapon index.
    pub fn put_unit(guest: &mut Guest, slot: u32, definition: u32, aim: [f32; 3], index: i8) {
        let body = body(slot);
        guest
            .set_u32(body + super::layout::BODY_DEFINITION, definition)
            .unwrap();
        for (i, value) in aim.into_iter().enumerate() {
            guest
                .set_f32(body + super::layout::UNIT_AIM + 4 * i as u32, value)
                .unwrap();
        }
        guest
            .set_u8(body + super::layout::UNIT_WEAPON_INDEX, index as u8)
            .unwrap();
        for i in 0..4 {
            guest
                .set_u32(body + super::layout::UNIT_WEAPONS + 4 * i, u32::MAX)
                .unwrap();
        }
    }

    /// A weapon object at `slot`, held by the unit at `unit_slot` in its
    /// weapon slot `index`.
    pub fn put_weapon(guest: &mut Guest, slot: u32, unit_slot: u32, index: u32, definition: u32) {
        put_object(guest, slot, object_datum(unit_slot));
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest
            .set_u8(
                header + super::layout::OBJECT_TYPE,
                super::layout::OBJECT_TYPE_WEAPON,
            )
            .unwrap();
        guest
            .set_u32(body(slot) + super::layout::BODY_DEFINITION, definition)
            .unwrap();
        guest
            .set_u32(
                body(unit_slot) + super::layout::UNIT_WEAPONS + 4 * index,
                object_datum(slot),
            )
            .unwrap();
        let high_water = OBJECTS + super::layout::DATUM_HIGH_WATER;
        let current = guest.u32(high_water).unwrap();
        guest.set_u32(high_water, current.max(slot + 1)).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_game_players_stats_and_objects() {
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        let variant = fixture::GLOBALS + layout::OPTIONS + layout::VARIANT;
        guest.set_u32(variant + layout::VARIANT_ENGINE, 2).unwrap();
        guest.set_u32(variant + layout::VARIANT_FLAGS, 1).unwrap();
        guest
            .set_u32(variant + layout::VARIANT_SCORE_LIMIT, 50)
            .unwrap();
        guest
            .set_u32(variant + layout::VARIANT_TIME_LIMIT, 600)
            .unwrap();
        guest
            .set_u32(fixture::TIME + layout::TIME_TICK, 120)
            .unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        let unit = fixture::object_datum(3);
        let vehicle = fixture::object_datum(4);
        fixture::put_player(&mut guest, 0, "Sneaky Pete", 1, true, unit);
        fixture::put_player(&mut guest, 2, "Remote", 1, false, u32::MAX);
        fixture::put_object(&mut guest, 3, vehicle);
        fixture::put_vehicle(&mut guest, 4);
        guest
            .set_u16(
                fixture::ENGINE
                    + layout::ENGINE_TEAM_STATS
                    + layout::TEAM_STAT_SIZE
                    + layout::STAT_SCORE,
                25,
            )
            .unwrap();

        // Whatever is at the difficulty word, multiplayer has none.
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_DIFFICULTY,
                2,
            )
            .unwrap();
        let info = game_info(&guest).unwrap();
        assert_eq!(info.map, "lockout");
        assert_eq!(info.mode, "multiplayer");
        assert_eq!(info.difficulty, None);
        assert_eq!(info.variant.as_deref(), Some("Slayer"));
        assert!(info.team_play);
        assert_eq!(info.score_limit, Some(50));
        assert_eq!(info.time_limit_s, Some(600));
        assert_eq!(info.tick, 120);

        let snapshot = snapshot(&guest).unwrap();
        assert_eq!(snapshot.players.len(), 2);
        let player = &snapshot.players[0];
        assert_eq!(player.player.name, "Sneaky Pete");
        assert_eq!(player.player.team, 1);
        assert_eq!(player.player.local, Some(true));
        assert_eq!(
            player.stats,
            PlayerStats {
                score: 10,
                kills: 1,
                deaths: 2,
                assists: 3,
                suicides: 4
            }
        );
        assert!(!snapshot.players[1].player.local.unwrap());
        assert_eq!(snapshot.teams, vec![TeamScore { team: 1, score: 25 }]);
        let object = snapshot.objects[0].as_ref().unwrap();
        assert_eq!(object.pos, [10.0, 20.0, 30.0]);
        assert_eq!(object.forward, [-1.0, 0.0, 0.0]);
        assert_eq!(object.vehicle, Some(vehicle));
        assert_eq!(yaw(object.forward), std::f64::consts::PI);
        assert!(snapshot.objects[1].is_none());
    }

    #[test]
    fn a_roster_read_after_another_keeps_unchanged_names_and_reads_new_ones() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "Sneaky Pete", 0, true, u32::MAX);
        fixture::put_player(&mut guest, 1, "B", 0, false, u32::MAX);
        let mut previous = players(&guest).unwrap();
        let kept = previous[0].player.name.as_ptr();
        // Slot 1 is renamed (one unit longer), slot 0 is not.
        fixture::put_player(&mut guest, 1, "Bo", 0, false, u32::MAX);
        let after = players_after(&guest, &mut previous).unwrap();
        assert_eq!(after, players(&guest).unwrap());
        assert_eq!(after[0].player.name, "Sneaky Pete");
        assert_eq!(after[0].player.name.as_ptr(), kept, "reused, not decoded");
        assert_eq!(after[1].player.name, "Bo");
        // A name another datum held is not taken for a new one.
        let mut stranger = after.clone();
        stranger[0].datum ^= 0x0001_0000;
        let renamed = players_after(&guest, &mut stranger).unwrap();
        assert_eq!(renamed[0].player.name, "Sneaky Pete");
        assert_eq!(stranger[0].player.name, "Sneaky Pete", "left where it was");
        assert_eq!(renamed, after);
    }

    #[test]
    fn a_bots_name_reads_without_its_mark() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "Sneaky Pete", 0, true, u32::MAX);
        fixture::put_player(&mut guest, 1, "\u{2699} Clanker 3", 1, false, u32::MAX);
        let read = players(&guest).unwrap();
        assert_eq!(read[0].player.name, "Sneaky Pete");
        assert_eq!(read[1].player.name, "Clanker 3");
        // Read again: the name is kept, not decoded afresh.
        let mut previous = read.clone();
        let kept = previous[1].player.name.as_ptr();
        let after = players_after(&guest, &mut previous).unwrap();
        assert_eq!(after[1].player.name.as_ptr(), kept);
    }

    #[test]
    fn skips_left_and_stale_players_and_bad_object_salts() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "A", 0, true, 0x90010000);
        fixture::put_player(&mut guest, 1, "B", 0, false, u32::MAX);
        let second = fixture::PLAYER_DATA + layout::PLAYER_SIZE;
        guest.set_u16(second + layout::PLAYER_FLAGS, 2).unwrap();
        guest
            .set_u16(fixture::OBJECT_DATA + layout::OBJECT_SALT, 1)
            .unwrap();
        assert_eq!(players(&guest).unwrap().len(), 1);
        assert!(object(&guest, 0x90020000).unwrap().is_none());
    }

    #[test]
    fn reads_local_players_with_their_controller_ports() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "Halo0001", 0, true, u32::MAX);
        fixture::put_player(&mut guest, 1, "Guest 2", 1, true, u32::MAX);
        fixture::put_player(&mut guest, 2, "Remote", 0, false, u32::MAX);
        let guest_slot = fixture::PLAYER_DATA + layout::PLAYER_SIZE;
        guest.set_u16(guest_slot + layout::PLAYER_LOCAL_USER, 1).unwrap();
        guest.set_u32(guest_slot + layout::PLAYER_CONTROLLER, 2).unwrap();
        // A remote player's controller field is -1; any value reads as none.
        guest
            .set_u32(
                fixture::PLAYER_DATA + 2 * layout::PLAYER_SIZE + layout::PLAYER_CONTROLLER,
                2,
            )
            .unwrap();

        let players = players(&guest).unwrap();
        assert_eq!(
            players
                .iter()
                .map(|p| (p.player.name.as_str(), p.local_user, p.controller))
                .collect::<Vec<_>>(),
            [
                ("Halo0001", Some(0), Some(0)),
                ("Guest 2", Some(1), Some(2)),
                ("Remote", None, None),
            ]
        );
        assert_eq!(players[1].player.index, 1, "keyed by its game slot");
        assert_eq!(players[1].player.local, Some(true));
        // An unbound local controller (-1) reads as none.
        guest
            .set_u32(guest_slot + layout::PLAYER_CONTROLLER, u32::MAX)
            .unwrap();
        assert_eq!(players_after(&guest, &mut []).unwrap()[1].controller, None);
    }

    #[test]
    fn reads_local_appearance_from_the_slot_profile() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "Local", 0, true, u32::MAX);
        fixture::put_player(&mut guest, 1, "Remote", 0, false, u32::MAX);
        fixture::put_slot_profile(&mut guest, 0, [5, 9, 2, 11], 30, 7);
        // A second local player joined through slot 2: the appearance comes
        // from that slot, not slot 0 (colour zero is a valid index).
        fixture::put_player(&mut guest, 2, "Guest", 0, true, u32::MAX);
        guest
            .set_u16(
                fixture::PLAYER_DATA + 2 * layout::PLAYER_SIZE + layout::PLAYER_LOCAL_USER,
                2,
            )
            .unwrap();
        fixture::put_slot_profile(&mut guest, 2, [0, 1, 2, 3], 0, 0);

        let players = players(&guest).unwrap();
        assert_eq!(
            players.iter().map(|p| p.local_user).collect::<Vec<_>>(),
            [Some(0), None, Some(2)],
            "the local slot is kept so the signed-in one can be linked"
        );
        assert_eq!(
            players[0].appearance,
            Appearance {
                primary: Some(5),
                secondary: Some(9),
                tertiary: Some(2),
                quaternary: Some(11),
                emblem_foreground: Some(30),
                emblem_background: Some(7),
            }
        );
        assert_eq!(players[1].appearance, Appearance::default());
        assert_eq!(
            players[2].appearance,
            Appearance {
                primary: Some(0),
                secondary: Some(1),
                tertiary: Some(2),
                quaternary: Some(3),
                emblem_foreground: Some(0),
                emblem_background: Some(0),
            }
        );
    }

    #[test]
    fn appearance_drops_out_of_range_values_and_unsigned_slots() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "Local", 0, true, u32::MAX);
        fixture::put_slot_profile(&mut guest, 0, [18, 0xff, 2, 11], 64, 31);

        let players = players(&guest).unwrap();
        assert_eq!(
            players[0].appearance,
            Appearance {
                tertiary: Some(2),
                quaternary: Some(11),
                emblem_background: Some(31),
                ..Appearance::default()
            }
        );

        // Without the signed-in flag nothing is read, bytes present or not.
        guest
            .set_u32(layout::PLAYER_SLOTS + layout::PLAYER_SLOT_FLAGS, 0)
            .unwrap();
        assert_eq!(
            super::players(&guest).unwrap()[0].appearance,
            Appearance::default()
        );
    }

    #[test]
    fn reads_unit_definition_aim_and_held_weapon() {
        let mut guest = fixture::memory();
        let unit = fixture::object_datum(1);
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_unit(&mut guest, 1, 0xe100_0010, [0.0, 0.6, 0.8], 1);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        fixture::put_weapon(&mut guest, 6, 1, 1, 0xe100_0021);

        let state = object(&guest, unit).unwrap().unwrap();
        assert_eq!(state.definition, Some(0xe100_0010));
        let aim = state.aim.unwrap();
        for (got, want) in aim.into_iter().zip([0.0, 0.6, 0.8]) {
            assert!((got - want).abs() < 1e-6, "{aim:?}");
        }
        assert_eq!(
            state.weapon,
            Some(Weapon {
                datum: fixture::object_datum(6),
                definition: 0xe100_0021,
            }),
            "the current index picks the slot"
        );

        // In a vehicle the transform is the vehicle's, but definition, aim
        // and weapon stay the unit's own.
        fixture::put_vehicle(&mut guest, 4);
        guest
            .set_u32(
                fixture::BODY + 0x1000 + layout::BODY_PARENT,
                fixture::object_datum(4),
            )
            .unwrap();
        let riding = object(&guest, unit).unwrap().unwrap();
        assert_eq!(riding.pos, [10.0, 20.0, 30.0]);
        assert_eq!(riding.definition, Some(0xe100_0010));
        assert_eq!(riding.weapon.unwrap().definition, 0xe100_0021);
    }

    #[test]
    fn garbage_aim_and_weapon_reads_are_dropped() {
        let mut guest = fixture::memory();
        let unit = fixture::object_datum(1);
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        let read = |guest: &runtime::Guest| object(guest, unit).unwrap().unwrap();
        for bad in [
            [0.0; 3],
            [f32::NAN, 0.0, 1.0],
            [3.0, 0.0, 0.0],
            [0.5, 0.0, 0.0],
        ] {
            fixture::put_unit(&mut guest, 1, 0xe100_0010, bad, 0);
            fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
            assert_eq!(read(&guest).aim, None, "{bad:?}");
        }
        // Small drift is renormalised.
        fixture::put_unit(&mut guest, 1, 0xe100_0010, [1.05, 0.0, 0.0], 0);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        assert_eq!(read(&guest).aim, Some([1.0, 0.0, 0.0]));
        assert!(read(&guest).weapon.is_some());

        // No weapon index, an index past the four slots, an empty slot.
        for index in [-1, 4, 2] {
            guest
                .set_u8(
                    fixture::BODY + 0x1000 + layout::UNIT_WEAPON_INDEX,
                    index as u8,
                )
                .unwrap();
            assert_eq!(read(&guest).weapon, None, "index {index}");
        }
        guest
            .set_u8(fixture::BODY + 0x1000 + layout::UNIT_WEAPON_INDEX, 0)
            .unwrap();
        // A weapon held by someone else, or not a weapon, is not this one's.
        guest
            .set_u32(
                fixture::BODY + 5 * 0x1000 + layout::BODY_PARENT,
                fixture::object_datum(2),
            )
            .unwrap();
        assert_eq!(read(&guest).weapon, None);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        guest
            .set_u8(
                fixture::OBJECT_DATA + 5 * layout::OBJECT_HEADER_SIZE + layout::OBJECT_TYPE,
                0,
            )
            .unwrap();
        assert_eq!(read(&guest).weapon, None);
        // A weapon body that faults drops the weapon, not the unit.
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        guest
            .set_u32(
                fixture::OBJECT_DATA + 5 * layout::OBJECT_HEADER_SIZE + layout::OBJECT_BODY,
                0xdead_0000,
            )
            .unwrap();
        let unit_state = read(&guest);
        assert_eq!(unit_state.weapon, None);
        assert_eq!(unit_state.pos, [1.0, 2.0, 3.0]);
        // A stale salt in the slot.
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        guest
            .set_u32(
                fixture::BODY + 0x1000 + layout::UNIT_WEAPONS,
                fixture::object_datum(5) ^ 0x0001_0000,
            )
            .unwrap();
        assert_eq!(read(&guest).weapon, None);
    }

    #[test]
    fn reads_the_tag_index_scenario() {
        let mut guest = fixture::memory();
        assert!(tag_index_scenario(&guest).is_err(), "no index yet");
        guest.set_u32(layout::TAG_INDEX_PTR, 0x16_0000).unwrap();
        guest
            .set_u32(0x16_0000 + layout::TAG_INDEX_SCENARIO, 0xe000_0001)
            .unwrap();
        assert_eq!(tag_index_scenario(&guest).unwrap(), 0xe000_0001);
    }

    #[test]
    fn finds_live_ai_bipeds_only() {
        let mut guest = fixture::memory();
        // 1: a player's biped; 2: a grunt; 3: a dead marine; 4: an unrelated
        // biped with no actor (a cinematic puppet); 5: a vehicle; 6: an
        // elite driving it; 7: a freed slot that was an actor.
        fixture::put_object(&mut guest, 1, u32::MAX);
        guest
            .set_u32(fixture::BODY + 0x1000 + layout::UNIT_PLAYER, 0x8001_0000)
            .unwrap();
        guest
            .set_u32(fixture::BODY + 0x1000 + layout::UNIT_ACTOR, 0xa000_0001)
            .unwrap();
        fixture::put_actor(&mut guest, 2, 3, [5.0, 6.0, 7.0]);
        fixture::put_actor(&mut guest, 3, 2, [0.0; 3]);
        guest
            .set_u16(
                fixture::BODY + 3 * 0x1000 + layout::BODY_DAMAGE_FLAGS,
                layout::BODY_DEPLETED,
            )
            .unwrap();
        fixture::put_object(&mut guest, 4, u32::MAX);
        fixture::put_vehicle(&mut guest, 5);
        guest
            .set_u32(
                fixture::BODY + 5 * 0x1000 + layout::BODY_DEFINITION,
                0xe100_0030,
            )
            .unwrap();
        fixture::put_actor(&mut guest, 6, 3, [0.0; 3]);
        guest
            .set_u32(
                fixture::BODY + 6 * 0x1000 + layout::BODY_PARENT,
                fixture::object_datum(5),
            )
            .unwrap();
        fixture::put_actor(&mut guest, 7, 3, [0.0; 3]);
        guest
            .set_u16(fixture::OBJECT_DATA + 7 * layout::OBJECT_HEADER_SIZE, 0)
            .unwrap();

        let found = actors(&guest).unwrap();
        assert_eq!(
            found.iter().map(|a| a.datum).collect::<Vec<_>>(),
            [fixture::object_datum(2), fixture::object_datum(6)]
        );
        // The dead marine is a body, not an actor.
        let bodies = world(&guest).unwrap().bodies;
        assert_eq!(
            bodies.iter().map(|a| a.datum).collect::<Vec<_>>(),
            [fixture::object_datum(3)]
        );
        assert_eq!(bodies[0].object.team, Some(2));
        let grunt = &found[0].object;
        assert_eq!(grunt.pos, [5.0, 6.0, 7.0]);
        assert_eq!(grunt.team, Some(3));
        assert_eq!(grunt.vehicle, None);
        assert_eq!(grunt.up, [0.0, 0.0, 1.0]);
        let driver = &found[1].object;
        assert_eq!(driver.vehicle, Some(fixture::object_datum(5)));
        assert_eq!(driver.vehicle_definition, Some(0xe100_0030));
        assert_eq!(driver.pos, [10.0, 20.0, 30.0], "the vehicle's transform");

        // A negative team is no team.
        guest
            .set_u16(fixture::BODY + 2 * 0x1000 + layout::UNIT_TEAM, u16::MAX)
            .unwrap();
        assert_eq!(actors(&guest).unwrap()[0].object.team, None);
    }

    #[test]
    fn actor_scan_skips_bipeds_whose_body_faults() {
        let mut guest = fixture::memory();
        fixture::put_actor(&mut guest, 1, 3, [0.0; 3]);
        fixture::put_actor(&mut guest, 2, 3, [0.0; 3]);
        guest
            .set_u32(
                fixture::OBJECT_DATA + layout::OBJECT_HEADER_SIZE + layout::OBJECT_BODY,
                0xdead_0000,
            )
            .unwrap();
        assert_eq!(
            actors(&guest)
                .unwrap()
                .iter()
                .map(|a| a.datum)
                .collect::<Vec<_>>(),
            [fixture::object_datum(2)]
        );
    }

    #[test]
    fn sim_snapshot_matches_live_except_physics_diagnostics() {
        let mut guest = fixture::memory();
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_player(&mut guest, 0, "A", 1, true, fixture::object_datum(1));
        let (array, component, bodies, rigid, movement, contacts) =
            (0x170000, 0x171000, 0x174000, 0x175000, 0x176000, 0x178000);
        for (address, value) in [
            (fixture::body(1) + layout::BODY_PHYSICS, 0),
            (layout::PHYSICS_COMPONENTS_PTR, array),
            (array + layout::DATUM_DATA, component),
            (component + layout::COMPONENT_BODIES, bodies),
            (bodies + layout::COMPONENT_BODY_RIGID, rigid),
            (rigid + layout::RIGID_MOTION, movement),
            (component + layout::COMPONENT_CHARACTER_CONTACTS, contacts),
            (component + layout::COMPONENT_CHARACTER_CONTACT_COUNT, 1),
        ] {
            guest.set_u32(address, value).unwrap();
        }
        let full = snapshot(&guest).unwrap();
        let mut expected = full.clone();
        let motion = &mut expected.objects[0].as_mut().unwrap().motion;
        assert_eq!(motion.support.take().unwrap().contacts.len(), 1);
        assert!(motion.havok.take().is_some());
        assert_eq!(sim_snapshot(&guest).unwrap(), expected);
        assert_eq!(snapshot(&guest).unwrap(), full);
    }

    #[test]
    fn a_snapshot_into_a_used_one_reads_what_a_fresh_one_does() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "A", 1, true, fixture::object_datum(1));
        fixture::put_player(&mut guest, 1, "B", 0, false, u32::MAX);
        fixture::put_player(&mut guest, 2, "C", 1, false, u32::MAX);
        fixture::put_object(&mut guest, 1, u32::MAX);
        let mut used = snapshot(&guest).unwrap();
        let mut spare = Vec::new();
        // Players leave and are renamed; teams and objects follow.
        let third = fixture::PLAYER_DATA + 2 * layout::PLAYER_SIZE;
        guest.set_u16(third + layout::PLAYER_FLAGS, 2).unwrap();
        fixture::put_player(&mut guest, 1, "Bo", 0, false, u32::MAX);
        let team = fixture::ENGINE + layout::ENGINE_TEAM_STATS + layout::TEAM_STAT_SIZE;
        guest.set_u16(team + layout::STAT_SCORE, 3).unwrap();
        guest
            .set_u32(fixture::TIME + layout::TIME_TICK, 77)
            .unwrap();
        snapshot_into(&guest, &mut used, &mut spare).unwrap();
        let fresh = snapshot(&guest).unwrap();
        assert_eq!(used, fresh);
        assert_eq!(used.players.len(), 2);
        assert_eq!(used.teams.len(), 2);
        assert_eq!(used.teams[1].score, 3);
        assert!(spare.is_empty(), "the old roster comes back empty");
    }

    #[test]
    fn placed_kinds_are_the_placement_blocks_kinds() {
        for t in 0..=u8::MAX {
            let tracked = layout::PLACEMENT_BLOCKS.iter().any(|(b, _, _)| *b == t);
            assert_eq!(placed_kind(t), object_kind(t).filter(|_| tracked), "{t}");
        }
    }

    #[test]
    fn winner_requires_a_unique_high_score() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "A", 0, true, u32::MAX);
        fixture::put_player(&mut guest, 1, "B", 0, false, u32::MAX);
        let winning = snapshot(&guest).unwrap();
        assert_eq!(
            winning.winner(false),
            Some(live_proto::game::Winner::Player { player: 1 })
        );
        let second = fixture::ENGINE + layout::ENGINE_PLAYER_STATS + layout::PLAYER_STAT_SIZE;
        guest.set_u16(second + layout::STAT_SCORE, 10).unwrap();
        let tied = snapshot(&guest).unwrap();
        assert_eq!(tied.winner(false), None);
    }

    #[test]
    fn campaign_skulls_come_from_the_array_and_multiplayer_has_none_there() {
        let mut guest = fixture::memory();
        let options = fixture::GLOBALS + layout::OPTIONS;
        guest.set_u32(options + layout::OPTIONS_TYPE, 1).unwrap();
        assert_eq!(game_info(&guest).unwrap().skulls, None, "none on");
        guest.set_u8(layout::SKULLS + 12, 1).unwrap();
        guest.set_u8(layout::SKULLS + 6, 1).unwrap();
        assert_eq!(game_info(&guest).unwrap().skulls, Some(vec![6, 12]));
        // The array is never cleared, but in multiplayer every reader skips
        // it: a leftover campaign skull is not this game's.
        guest.set_u32(options + layout::OPTIONS_TYPE, 2).unwrap();
        assert_eq!(game_info(&guest).unwrap().skulls, None);
    }

    #[test]
    fn campaign_difficulty_comes_from_the_options() {
        let mut guest = fixture::memory();
        let options = fixture::GLOBALS + layout::OPTIONS;
        guest.set_u32(options + layout::OPTIONS_TYPE, 1).unwrap();
        for (value, name) in [
            (0, Some("easy")),
            (1, Some("normal")),
            (2, Some("heroic")),
            (3, Some("legendary")),
            (4, None),
            (0xffff, None),
        ] {
            guest
                .set_u16(options + layout::OPTIONS_DIFFICULTY, value)
                .unwrap();
            let info = game_info(&guest).unwrap();
            assert_eq!(info.mode, "campaign");
            assert_eq!(info.difficulty.as_deref(), name, "{value}");
        }
    }

    #[test]
    fn world_scan_finds_vehicles_machines_and_who_rides_what() {
        let mut guest = fixture::memory();
        fixture::put_vehicle(&mut guest, 1);
        fixture::put_vehicle(&mut guest, 2);
        fixture::put_actor(&mut guest, 3, 3, [1.0, 2.0, 3.0]);
        // The actor drives vehicle 2; vehicle 1 is empty.
        guest
            .set_u32(
                fixture::BODY + 3 * 0x1000 + layout::BODY_PARENT,
                fixture::object_datum(2),
            )
            .unwrap();
        fixture::put_machine(&mut guest, 4, 7, 0.25);
        fixture::put_machine(&mut guest, 5, 8, 3.0);
        fixture::put_machine(&mut guest, 6, -1, 0.5);
        fixture::put_machine(&mut guest, 7, 9, 0.5);
        fixture::put_machine(&mut guest, 8, 10, 0.5);
        guest.set_u32(fixture::body(7) + 0x18c, u32::MAX).unwrap();
        guest.set_u16(fixture::body(8) + 0x192, u16::MAX).unwrap();
        // Machine power is separate from its position.
        guest.set_f32(fixture::body(4) + 0x134, 0.75).unwrap();
        let world = world(&guest).unwrap();
        assert_eq!(world.actors.len(), 1);
        let occupied: Vec<_> = world
            .vehicles
            .iter()
            .map(|v| (v.datum, v.occupied))
            .collect();
        assert_eq!(
            occupied,
            [
                (fixture::object_datum(1), false),
                (fixture::object_datum(2), true)
            ]
        );
        assert_eq!(world.vehicles[0].pos, [10.0, 20.0, 30.0]);
        assert_eq!(world.machines.len(), 5);
        assert_eq!(world.machines[0].placement, Some(7));
        assert_eq!(world.machines[0].position, Some(0.25));
        assert_eq!(world.machines[1].position, None, "out of range");
        assert_eq!(world.machines[2].placement, None, "not placed");
        assert_eq!(world.machines[2].position, None);
        assert_eq!(world.machines[3].position, None, "no graph");
        assert_eq!(world.machines[4].position, None, "no position animation");
        assert_eq!(world.machines[0].up, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_turret_knows_its_vehicle_and_aim_and_its_gunner_is_where_the_vehicle_is() {
        let mut guest = fixture::memory();
        // Vehicle 1 (the Warthog) carries turret 2 (its chaingun), whose
        // transform is relative to the Warthog; actor 3 mans the turret.
        fixture::put_vehicle(&mut guest, 1);
        fixture::put_vehicle(&mut guest, 2);
        let turret = fixture::BODY + 2 * 0x1000;
        guest
            .set_u32(turret + layout::BODY_PARENT, fixture::object_datum(1))
            .unwrap();
        for (i, v) in [-0.6f32, 0.0, 0.75].into_iter().enumerate() {
            guest
                .set_f32(turret + layout::BODY_POSITION + 4 * i as u32, v)
                .unwrap();
        }
        for (i, v) in [0.0f32, 0.6, 0.8].into_iter().enumerate() {
            guest
                .set_f32(turret + layout::UNIT_AIM + 4 * i as u32, v)
                .unwrap();
        }
        fixture::put_actor(&mut guest, 3, 3, [0.0; 3]);
        guest
            .set_u32(
                fixture::BODY + 3 * 0x1000 + layout::BODY_PARENT,
                fixture::object_datum(2),
            )
            .unwrap();
        let world = world(&guest).unwrap();
        let by = |slot| {
            world
                .vehicles
                .iter()
                .find(|v| v.datum == fixture::object_datum(slot))
                .unwrap()
        };
        assert_eq!(by(1).parent, None);
        assert_eq!(by(2).parent, Some(fixture::object_datum(1)));
        let aim = by(2).aim.unwrap();
        for (got, want) in aim.into_iter().zip([0.0, 0.6, 0.8]) {
            assert!((got - want).abs() < 1e-6, "{aim:?}");
        }
        // A vehicle with no aim written has none (zero is not a direction).
        assert_eq!(by(1).aim, None);
        let gunner = &world.actors[0].object;
        assert_eq!(gunner.vehicle, Some(fixture::object_datum(2)));
        assert_eq!(gunner.mount, Some(fixture::object_datum(1)));
        assert_eq!(
            gunner.pos,
            [10.0, 20.0, 30.0],
            "the Warthog's, not the turret's own"
        );
        assert_eq!(gunner.forward, [-1.0, 0.0, 0.0]);
    }

    #[test]
    fn unit_motion_fields() {
        let mut guest = fixture::memory();
        let unit = fixture::object_datum(1);
        fixture::put_object(&mut guest, 1, u32::MAX);
        let body = fixture::BODY + 0x1000;
        guest.set_f32(body + layout::UNIT_CROUCH, 0.75).unwrap();
        guest.set_u8(body + layout::BIPED_PHYSICS_MODE, 2).unwrap();
        guest
            .set_f32(body + layout::BODY_VELOCITY + 8, 0.5)
            .unwrap();
        guest.set_u8(body + layout::UNIT_GRENADE_INDEX, 1).unwrap();
        guest.set_u8(body + layout::UNIT_GRENADE_COUNTS, 2).unwrap();
        guest
            .set_u8(body + layout::UNIT_GRENADE_COUNTS + 1, 1)
            .unwrap();
        // A held weapon with a magazine.
        fixture::put_object(&mut guest, 2, unit);
        guest
            .set_u8(
                fixture::OBJECT_DATA + 2 * layout::OBJECT_HEADER_SIZE + layout::OBJECT_TYPE,
                layout::OBJECT_TYPE_WEAPON,
            )
            .unwrap();
        let weapon = fixture::BODY + 2 * 0x1000;
        guest
            .set_u32(weapon + layout::BODY_DEFINITION, 0xe000_0001)
            .unwrap();
        guest
            .set_u16(weapon + layout::WEAPON_MAGAZINE_RESERVE, 96)
            .unwrap();
        guest
            .set_u16(weapon + layout::WEAPON_MAGAZINE_LOADED, 32)
            .unwrap();
        guest.set_u8(body + layout::UNIT_WEAPON_INDEX, 0).unwrap();
        guest
            .set_u32(body + layout::UNIT_WEAPONS, fixture::object_datum(2))
            .unwrap();
        for i in 1..4 {
            guest
                .set_u32(body + layout::UNIT_WEAPONS + 4 * i, u32::MAX)
                .unwrap();
        }
        guest
            .set_u32(body + layout::UNIT_FLAGS, layout::UNIT_FLAG_CROUCH | 1)
            .unwrap();
        guest.set_u16(body + layout::BIPED_FLAGS, 0x0201).unwrap();
        guest
            .set_u8(body + layout::BIPED_HITBOX_OFFSET, (-44i8) as u8)
            .unwrap();
        guest
            .set_u8(body + layout::BIPED_AIRBORNE_TICKS, 9)
            .unwrap();
        // A Havok body: component 3 of the array, its first rigid body.
        let (array, data, bodies, rigid, motion) =
            (0x17_0000, 0x17_1000, 0x17_4000, 0x17_5000, 0x17_6000);
        guest
            .set_u32(body + layout::BODY_PHYSICS, 0xe003_0003)
            .unwrap();
        guest
            .set_u32(layout::PHYSICS_COMPONENTS_PTR, array)
            .unwrap();
        guest.set_u32(array + layout::DATUM_DATA, data).unwrap();
        guest
            .set_u32(
                data + 3 * layout::PHYSICS_COMPONENT_SIZE + layout::COMPONENT_BODIES,
                bodies,
            )
            .unwrap();
        guest
            .set_u32(bodies + layout::COMPONENT_BODY_RIGID, rigid)
            .unwrap();
        guest.set_u32(rigid + layout::RIGID_MOTION, motion).unwrap();
        guest
            .set_f32(motion + layout::MOTION_POSITION + 8, -3.25)
            .unwrap();
        guest
            .set_f32(motion + layout::MOTION_VELOCITY + 8, 12.5)
            .unwrap();
        // Ground support: a copied support contact on structure surface
        // 0x0b11, and two character contacts on the component. The
        // support byte is the high byte of the biped flags (0x0201 above),
        // so setting its copied bit makes them 0x1201.
        guest
            .set_u8(
                body + layout::BIPED_SUPPORT_FLAGS,
                0x02 | layout::BIPED_SUPPORT_COPIED,
            )
            .unwrap();
        for (i, w) in [0x0002u32, u32::MAX, 0x0b11, u32::MAX, 0x2000_0b11]
            .iter()
            .enumerate()
        {
            guest
                .set_u32(body + layout::BIPED_SUPPORT_WORDS + 4 * i as u32, *w)
                .unwrap();
        }
        guest
            .set_f32(body + layout::BIPED_SUPPORT_POINT + 8, 1.25)
            .unwrap();
        guest
            .set_f32(body + layout::BIPED_SUPPORT_NORMAL + 8, 0.75)
            .unwrap();
        guest
            .set_f32(body + layout::BIPED_SUPPORT_CONTACT_POINT, 43.5)
            .unwrap();
        guest
            .set_f32(body + layout::BIPED_GROUND_STATE + 4, -2.0)
            .unwrap();
        let (list, component) = (0x17_8000, data + 3 * layout::PHYSICS_COMPONENT_SIZE);
        guest
            .set_u32(component + layout::COMPONENT_CHARACTER_CONTACTS, list)
            .unwrap();
        guest
            .set_u32(component + layout::COMPONENT_CHARACTER_CONTACT_COUNT, 2)
            .unwrap();
        let second = list + layout::CHARACTER_CONTACT_SIZE;
        guest
            .set_u32(second + layout::CHARACTER_CONTACT_KEY, 0x2000_0b11)
            .unwrap();
        guest
            .set_u32(second + layout::CHARACTER_CONTACT_OTHER, u32::MAX)
            .unwrap();
        guest
            .set_f32(second + layout::CHARACTER_CONTACT_POINT + 4, -79.25)
            .unwrap();
        guest
            .set_f32(second + layout::CHARACTER_CONTACT_NORMAL + 8, 0.5)
            .unwrap();
        guest
            .set_f32(second + layout::CHARACTER_CONTACT_VALUE, 0.125)
            .unwrap();
        guest
            .set_u32(second + layout::CHARACTER_CONTACT_FLAGS, 6)
            .unwrap();
        let m = object(&guest, unit).unwrap().unwrap().motion;
        let s = m.support.clone().expect("a biped's support");
        assert_eq!(s.flags, 0x12);
        assert_eq!(s.words, [0x0002, u32::MAX, 0x0b11, u32::MAX, 0x2000_0b11]);
        assert_eq!(s.point, [0.0, 0.0, 1.25]);
        assert_eq!(s.normal, [0.0, 0.0, 0.75]);
        assert_eq!(s.contact_point, [43.5, 0.0, 0.0]);
        assert_eq!(s.ground_state, [0.0, -2.0, 0.0]);
        assert_eq!(s.contacts.len(), 2);
        assert_eq!(s.contacts[0], CharacterContact::default());
        assert_eq!(
            s.contacts[1],
            CharacterContact {
                key: 0x2000_0b11,
                other: u32::MAX,
                point: [0.0, -79.25, 0.0],
                normal: [0.0, 0.0, 0.5],
                value: 0.125,
                flags: 6,
            }
        );
        // A count past the cap reads no contacts rather than garbage.
        guest
            .set_u32(
                component + layout::COMPONENT_CHARACTER_CONTACT_COUNT,
                layout::CHARACTER_CONTACTS_MAX + 1,
            )
            .unwrap();
        let capped = object(&guest, unit).unwrap().unwrap().motion;
        assert!(capped.support.unwrap().contacts.is_empty());
        assert_eq!(m.havok, Some(([0.0, 0.0, -3.25], [0.0, 0.0, 12.5])));
        assert_eq!(m.unit_flags, Some(layout::UNIT_FLAG_CROUCH | 1));
        assert_eq!(
            m.biped_flags,
            Some(0x1201),
            "the support byte is their high byte"
        );
        assert_eq!(m.airborne_ticks, Some(9));
        assert_eq!(m.hitbox_offset, Some(-44));
        assert_eq!(m.crouch, Some(0.75));
        assert_eq!(m.physics_mode, Some(2));
        assert_eq!(m.velocity, [0.0, 0.0, 0.5]);
        assert_eq!(m.grenade, Some(1));
        assert_eq!(m.grenades, Some(3));
        assert_eq!(m.magazine, Some((96, 32)));
        assert_eq!(m.animation, None, "no manager");
        // The manager at +0x300, playing animation 5 of the graph's
        // second parent at frame 12.5.
        guest
            .set_u16(body + layout::OBJECT_ANIMATION_MANAGER, 0x300)
            .unwrap();
        let channel = body + 0x300 + layout::MANAGER_CHANNEL;
        guest
            .set_u32(channel + layout::CHANNEL_GRAPH, 0xe123_0456)
            .unwrap();
        guest
            .set_u16(channel + layout::CHANNEL_ANIMATION, 1)
            .unwrap();
        guest
            .set_u16(channel + layout::CHANNEL_ANIMATION + 2, 5)
            .unwrap();
        guest
            .set_f32(channel + layout::CHANNEL_FRAME, 12.5)
            .unwrap();
        let m = object(&guest, unit).unwrap().unwrap().motion;
        assert_eq!(
            m.animation,
            Some(Animation {
                graph: 0xe123_0456,
                inherited: 1,
                index: 5,
                frame: 12.5
            })
        );
        // No animation, a nonsense frame, no manager: nothing.
        guest
            .set_u16(channel + layout::CHANNEL_ANIMATION + 2, 0xffff)
            .unwrap();
        assert_eq!(animation(&guest, body).unwrap(), None);
        guest
            .set_u16(channel + layout::CHANNEL_ANIMATION + 2, 5)
            .unwrap();
        guest
            .set_f32(channel + layout::CHANNEL_FRAME, f32::NAN)
            .unwrap();
        assert_eq!(animation(&guest, body).unwrap(), None);
        guest.set_f32(channel + layout::CHANNEL_FRAME, 1.0).unwrap();
        guest
            .set_u16(body + layout::OBJECT_ANIMATION_MANAGER, 0xffff)
            .unwrap();
        assert_eq!(animation(&guest, body).unwrap(), None);
        assert_eq!(
            m.weapons,
            Some([fixture::object_datum(2), u32::MAX, u32::MAX, u32::MAX])
        );
        // Out-of-range values read as unknown.
        guest.set_f32(body + layout::UNIT_CROUCH, 7.0).unwrap();
        guest
            .set_u8(body + layout::BIPED_PHYSICS_MODE, 0x40)
            .unwrap();
        guest
            .set_u8(body + layout::UNIT_GRENADE_INDEX, 0xff)
            .unwrap();
        let m = object(&guest, unit).unwrap().unwrap().motion;
        assert_eq!((m.crouch, m.physics_mode, m.grenade), (None, None, None));
    }

    #[test]
    fn the_cinematic_byte_says_whether_a_cutscene_plays() {
        let mut guest = fixture::memory();
        fixture::put_player(&mut guest, 0, "A", 1, true, u32::MAX);
        // No block yet (before the game state is set up): no cinematic.
        assert!(!cinematic(&guest).unwrap());
        assert!(!snapshot(&guest).unwrap().cinematic);
        fixture::set_cinematic(&mut guest, true);
        assert!(cinematic(&guest).unwrap());
        assert!(snapshot(&guest).unwrap().cinematic);
        let mut out = Snapshot::default();
        snapshot_into(&guest, &mut out, &mut Vec::new()).unwrap();
        assert!(out.cinematic);
        // `+4` (also set by cinematic_start) is not the one read.
        guest
            .set_u8(fixture::CINEMATIC + layout::CINEMATIC_IN_PROGRESS, 0)
            .unwrap();
        guest.set_u8(fixture::CINEMATIC + 4, 1).unwrap();
        assert!(!cinematic(&guest).unwrap());
        snapshot_into(&guest, &mut out, &mut Vec::new()).unwrap();
        assert!(!out.cinematic);
    }

    #[test]
    fn unit_velocity_seat_and_vitality() {
        let mut guest = fixture::memory();
        let unit = fixture::object_datum(1);
        fixture::put_object(&mut guest, 1, u32::MAX);
        let body = fixture::BODY + 0x1000;
        for (i, v) in [3.0f32, -4.0, 0.5].into_iter().enumerate() {
            guest
                .set_f32(body + layout::BODY_VELOCITY + 4 * i as u32, v)
                .unwrap();
        }
        guest.set_u16(body + layout::UNIT_SEAT, u16::MAX).unwrap();
        guest.set_f32(body + layout::BODY_VITALITY, 0.75).unwrap();
        guest
            .set_f32(body + layout::BODY_SHIELD_VITALITY, 2.5)
            .unwrap();
        let o = object(&guest, unit).unwrap().unwrap();
        assert_eq!(o.velocity, Some([3.0, -4.0, 0.5]), "world units a second");
        assert_eq!(o.seat, None, "-1: in no seat");
        assert_eq!(o.vitality, Some((0.75, 2.5)));
        // A depleted body has none left, as the game's getters say.
        guest
            .set_u16(body + layout::BODY_DAMAGE_FLAGS, layout::BODY_DEPLETED)
            .unwrap();
        let o = object(&guest, unit).unwrap().unwrap();
        assert_eq!(o.vitality, Some((0.0, 0.0)));
        guest.set_u16(body + layout::BODY_DAMAGE_FLAGS, 0).unwrap();
        guest
            .set_f32(body + layout::BODY_SHIELD_VITALITY, f32::NAN)
            .unwrap();
        let o = object(&guest, unit).unwrap().unwrap();
        assert_eq!(o.vitality, None, "not finite: not read");
        // In a vehicle's third seat: the seat, and the vehicle's velocity
        // (its transform is where the rider is).
        fixture::put_vehicle(&mut guest, 2);
        let vehicle = fixture::BODY + 2 * 0x1000;
        for (i, v) in [10.0f32, 0.0, -1.0].into_iter().enumerate() {
            guest
                .set_f32(vehicle + layout::BODY_VELOCITY + 4 * i as u32, v)
                .unwrap();
        }
        guest
            .set_u32(body + layout::BODY_PARENT, fixture::object_datum(2))
            .unwrap();
        guest.set_u16(body + layout::UNIT_SEAT, 2).unwrap();
        let o = object(&guest, unit).unwrap().unwrap();
        assert_eq!(o.vehicle, Some(fixture::object_datum(2)));
        assert_eq!(o.seat, Some(2));
        assert_eq!(o.velocity, Some([10.0, 0.0, -1.0]));
        assert_eq!(o.motion.velocity, [3.0, -4.0, 0.5], "motion keeps its own");
        // A velocity that is not finite is not read.
        guest
            .set_f32(vehicle + layout::BODY_VELOCITY, f32::INFINITY)
            .unwrap();
        let o = object(&guest, unit).unwrap().unwrap();
        assert_eq!(o.velocity, None);
    }

    #[test]
    fn active_bsp_and_damage_reports() {
        let mut guest = fixture::memory();
        guest
            .set_u16(fixture::GLOBALS + layout::GLOBALS_ACTIVE_BSP, 2)
            .unwrap();
        assert_eq!(active_bsp(&guest).unwrap(), Some(2));
        guest
            .set_u16(fixture::GLOBALS + layout::GLOBALS_ACTIVE_BSP, 0xffff)
            .unwrap();
        assert_eq!(active_bsp(&guest).unwrap(), None);

        fixture::put_object(&mut guest, 1, u32::MAX);
        let report = 0x1f0000;
        guest
            .set_u8(report + layout::REPORT_CODE, 0x80 | 10)
            .unwrap();
        guest
            .set_u32(report + layout::REPORT_EFFECT, 0xe100_0005)
            .unwrap();
        guest
            .set_u32(report + layout::REPORT_OWNER_PLAYER, 0x8002_0002)
            .unwrap();
        guest
            .set_u32(report + layout::REPORT_OWNER_OBJECT, 0x9009_0009)
            .unwrap();
        guest.set_f32(report + layout::REPORT_BODY, 0.25).unwrap();
        guest.set_f32(report + layout::REPORT_SHIELD, 0.5).unwrap();
        let hit = hit(&guest, report, fixture::object_datum(1)).unwrap();
        assert_eq!(hit.code & 0x3f, 10);
        assert_eq!(hit.effect, 0xe100_0005);
        assert_eq!(hit.owner_player, 0x8002_0002);
        assert_eq!(hit.owner_object, 0x9009_0009);
        assert_eq!((hit.body, hit.shield), (0.25, 0.5));
        assert!(hit.pos.is_some());
        let gone = super::hit(&guest, report, 0x1234_0077).unwrap();
        assert_eq!(gone.pos, None, "a dead datum has no position");
    }

    #[test]
    fn world_scan_notes_placed_and_netgame_objects() {
        let mut guest = fixture::memory();
        // A crate from crate placement 4, a weapon on netgame spawn 2 that
        // also came from weapon placement 7, a held weapon from placement
        // 1, a dynamic weapon, and a machine (frames carry those apart).
        fixture::put_placed(&mut guest, 1, 11, 4, -1, [5.0, 6.0, 7.0], u32::MAX);
        fixture::put_placed(&mut guest, 2, 2, 7, 2, [1.0, 1.0, 1.0], u32::MAX);
        fixture::put_placed(&mut guest, 3, 2, 1, -1, [0.0, 0.0, 0.0], 0x9001_0001);
        fixture::put_placed(&mut guest, 4, 2, -1, -1, [0.0, 0.0, 0.0], u32::MAX);
        fixture::put_machine(&mut guest, 5, 3, 0.5);
        let placed = world(&guest).unwrap().placed;
        let seen: Vec<_> = placed
            .iter()
            .map(|p| (p.kind, p.index, p.parented))
            .collect();
        assert_eq!(
            seen,
            [
                (ObjectKind::Crate, 4, false),
                (ObjectKind::Weapon, 7, false),
                (ObjectKind::NetgameEquipment, 2, false),
                (ObjectKind::Weapon, 1, true),
            ]
        );
        assert_eq!(placed[0].pos, [5.0, 6.0, 7.0]);
        assert_eq!(placed[0].forward, [0.0, 1.0, 0.0]);
        assert_eq!(placed[0].up, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn things_lists_objects_of_the_wanted_types_with_their_tag_and_pose() {
        let mut guest = fixture::memory();
        // Scenery from placement 3, a weapon on the ground, a weapon a
        // unit holds, a dynamic equipment, and a vehicle (not wanted).
        fixture::put_placed(&mut guest, 1, 6, 3, -1, [5.0, 6.0, 7.0], u32::MAX);
        fixture::put_placed(&mut guest, 2, 2, -1, -1, [1.0, 1.0, 1.0], u32::MAX);
        let holder = fixture::object_datum(9);
        fixture::put_placed(&mut guest, 3, 2, -1, -1, [0.0, 0.0, 0.0], holder);
        fixture::put_placed(&mut guest, 4, 3, -1, -1, [2.0, 0.0, 0.0], u32::MAX);
        fixture::put_vehicle(&mut guest, 5);
        guest
            .set_u32(fixture::body(1) + layout::BODY_DEFINITION, 0xe123_0456)
            .unwrap();
        let found = things(&guest, |t| matches!(t, 2 | 3 | 6)).unwrap();
        let seen: Vec<_> = found
            .iter()
            .map(|t| (t.datum, t.object_type, t.parent, t.placement))
            .collect();
        assert_eq!(
            seen,
            [
                (fixture::object_datum(1), 6, None, Some(3)),
                (fixture::object_datum(2), 2, None, None),
                (fixture::object_datum(3), 2, Some(holder), None),
                (fixture::object_datum(4), 3, None, None),
            ]
        );
        assert_eq!(found[0].definition, Some(0xe123_0456));
        assert_eq!(found[0].pos, [5.0, 6.0, 7.0]);
        assert_eq!(found[0].forward, [0.0, 1.0, 0.0]);
        assert_eq!(found[0].up, [0.0, 0.0, 1.0]);
        assert!(things(&guest, |_| false).unwrap().is_empty());
    }

    #[test]
    fn world_scan_finds_projectiles_and_loose_items() {
        let mut guest = fixture::memory();
        let weapon = layout::OBJECT_TYPE_WEAPON;
        // A weapon from a placement, a held one, a dropped one and dropped
        // equipment: only the last two are loose.
        fixture::put_placed(&mut guest, 1, weapon, 7, -1, [1.0, 1.0, 1.0], u32::MAX);
        let held_by = fixture::object_datum(6);
        fixture::put_placed(&mut guest, 2, weapon, -1, -1, [0.0; 3], held_by);
        fixture::put_placed(&mut guest, 3, weapon, -1, -1, [4.0, 5.0, 6.0], u32::MAX);
        let equipment = layout::OBJECT_TYPE_EQUIPMENT;
        fixture::put_placed(&mut guest, 4, equipment, -1, -1, [7.0, 8.0, 9.0], u32::MAX);
        // A grenade in flight, and one stuck to the biped at slot 6 (its
        // own transform parent-relative).
        let projectile = layout::OBJECT_TYPE_PROJECTILE;
        fixture::put_placed(&mut guest, 5, projectile, -1, -1, [2.0, 3.0, 4.0], u32::MAX);
        let flying = fixture::body(5) + layout::BODY_VELOCITY;
        guest.set_f32(flying, 5.0).unwrap();
        guest.set_f32(flying + 8, -1.5).unwrap();
        fixture::put_object(&mut guest, 6, u32::MAX);
        fixture::put_placed(&mut guest, 7, projectile, -1, -1, [0.1, 0.0, 0.2], held_by);
        let world = world(&guest).unwrap();
        let items: Vec<_> = world
            .items
            .iter()
            .map(|i| (i.datum, i.equipment, i.pos))
            .collect();
        assert_eq!(
            items,
            [
                (fixture::object_datum(3), false, [4.0, 5.0, 6.0]),
                (fixture::object_datum(4), true, [7.0, 8.0, 9.0]),
            ]
        );
        assert_eq!(world.items[0].forward, [0.0, 1.0, 0.0]);
        assert_eq!(world.items[0].up, [0.0, 0.0, 1.0]);
        let projectiles: Vec<_> = world
            .projectiles
            .iter()
            .map(|p| (p.datum, p.pos, p.velocity, p.parent))
            .collect();
        assert_eq!(
            projectiles,
            [
                (
                    fixture::object_datum(5),
                    [2.0, 3.0, 4.0],
                    Some([5.0, 0.0, -1.5]),
                    None
                ),
                (
                    fixture::object_datum(7),
                    [1.0, 2.0, 3.0],
                    None,
                    Some(held_by)
                ),
            ],
            "a stuck grenade is where it stuck"
        );
        // The vehicle scan reads velocity too.
        fixture::put_vehicle(&mut guest, 8);
        guest
            .set_f32(fixture::body(8) + layout::BODY_VELOCITY + 4, 7.5)
            .unwrap();
        let world = super::world(&guest).unwrap();
        assert_eq!(world.vehicles[0].velocity, Some([0.0, 7.5, 0.0]));
    }

    #[test]
    fn placements_read_scenario_blocks_and_netgame_equipment() {
        let mut guest = fixture::memory();
        let scenario = 0x180000;
        guest.set_u32(layout::SCENARIO_PTR, scenario).unwrap();
        let put_block = |guest: &mut Guest,
                         at: u32,
                         records: u32,
                         poses: &[([f32; 3], [f32; 3])],
                         stride: u32,
                         position: u32,
                         rotation: u32| {
            guest.set_u32(scenario + at, poses.len() as u32).unwrap();
            guest.set_u32(scenario + at + 4, records).unwrap();
            for (i, (pos, rot)) in poses.iter().enumerate() {
                let record = records + i as u32 * stride;
                for k in 0..3 {
                    guest
                        .set_f32(record + position + 4 * k as u32, pos[k])
                        .unwrap();
                    guest
                        .set_f32(record + rotation + 4 * k as u32, rot[k])
                        .unwrap();
                }
            }
        };
        let half_pi = std::f32::consts::FRAC_PI_2;
        // Two crates: one unrotated, one turned a quarter left (yaw); the
        // second's record holds a NaN and does not read.
        let (_, crate_at, crate_stride) = *layout::PLACEMENT_BLOCKS
            .iter()
            .find(|(t, _, _)| *t == 11)
            .unwrap();
        put_block(
            &mut guest,
            crate_at,
            0x190000,
            &[
                ([1.0, 2.0, 3.0], [half_pi, 0.0, 0.0]),
                ([f32::NAN, 0.0, 0.0], [0.0; 3]),
            ],
            crate_stride,
            layout::PLACEMENT_POSITION,
            layout::PLACEMENT_ROTATION,
        );
        put_block(
            &mut guest,
            layout::NETGAME_EQUIPMENT,
            0x1a0000,
            &[([9.0, 8.0, 7.0], [0.0; 3])],
            layout::NETGAME_EQUIPMENT_STRIDE,
            layout::NETGAME_EQUIPMENT_POSITION,
            layout::NETGAME_EQUIPMENT_ROTATION,
        );
        // An absurd count is a bad block, not an allocation.
        let (_, weapon_at, _) = *layout::PLACEMENT_BLOCKS
            .iter()
            .find(|(t, _, _)| *t == 2)
            .unwrap();
        guest.set_u32(scenario + weapon_at, 0x10_0000).unwrap();
        guest.set_u32(scenario + weapon_at + 4, 0x190000).unwrap();
        let placements = placements(&guest).unwrap();
        let kinds: Vec<_> = placements.kinds.iter().map(|(k, _)| *k).collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        assert_eq!(kinds, sorted, "placements are in kind order");
        assert_eq!(kinds.last(), Some(&ObjectKind::NetgameEquipment));
        assert!(!kinds.contains(&ObjectKind::Machine));
        let block = |kind| &placements.kinds.iter().find(|(k, _)| *k == kind).unwrap().1;
        let crates = block(ObjectKind::Crate);
        assert_eq!(crates.len(), 2);
        let first = crates[0].unwrap();
        assert_eq!(first.pos, [1.0, 2.0, 3.0]);
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert!(near(first.forward, [0.0, 1.0, 0.0]), "{:?}", first.forward);
        assert!(near(first.up, [0.0, 0.0, 1.0]));
        assert_eq!(crates[1], None);
        assert!(block(ObjectKind::Weapon).is_empty());
        assert!(block(ObjectKind::Scenery).is_empty());
        let netgame = block(ObjectKind::NetgameEquipment);
        assert_eq!(netgame.len(), 1);
        assert_eq!(netgame[0].unwrap().pos, [9.0, 8.0, 7.0]);
        assert_eq!(netgame[0].unwrap().forward, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn rotation_axes_follow_yaw_pitch_roll() {
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
        let q = std::f64::consts::FRAC_PI_2;
        assert!(near(rotation_axes([0.0; 3]).0, [1.0, 0.0, 0.0]));
        // Pitch up a quarter: forward points up, up points back.
        let (f, u) = rotation_axes([0.0, q, 0.0]);
        assert!(near(f, [0.0, 0.0, 1.0]), "{f:?}");
        assert!(near(u, [-1.0, 0.0, 0.0]), "{u:?}");
        // Roll a quarter: up tips to -y, forward stays.
        let (f, u) = rotation_axes([0.0, 0.0, q]);
        assert!(near(f, [1.0, 0.0, 0.0]));
        assert!(near(u, [0.0, -1.0, 0.0]), "{u:?}");
    }
}
