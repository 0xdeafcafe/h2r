//! Retail game-engine objective registration and scenario scoring zones.

use guest_layout::{DatumHandle, GuestMemory, MemFault, points};

/// Holds the game-engine globals' address (`0xc9c` bytes).
pub const GAME_ENGINE: u32 = points::GAME_ENGINE.base;
/// `E+0xc14`: the engine index in play (`i32`; `0x1574c0` copies the
/// variant's `+0x44` there and sets 0 when that engine has no globals).
pub const ENGINE_INDEX: u32 = 0xc14;
/// `E+0xc18`: how many objective objects are registered (`i32`, at most
/// [`OBJECTIVE_MAX`]).
pub const OBJECTIVE_COUNT: u32 = 0xc18;
/// `E+0xc1c`: the objective records, [`OBJECTIVE_STRIDE`] each.
pub const OBJECTIVES: u32 = 0xc1c;
pub const OBJECTIVE_STRIDE: u32 = 0x10;
pub const OBJECTIVE_MAX: u32 = 8;
/// In a record: the object (`u32` datum), its team (`i16`; a ball's
/// number in Oddball), the unit carrying it and that unit's player (`u32`
/// datums, -1 for none).
pub const RECORD_OBJECT: u32 = 0;
pub const RECORD_TEAM: u32 = 4;
pub const RECORD_UNIT: u32 = 8;
pub const RECORD_PLAYER: u32 = 0xc;
/// `E+8`: the netgame team designators in use (`u16` mask, bit per
/// designator); `E+0x10`: each designator's game team (`i16`, 9 of them).
pub const DESIGNATOR_MASK: u32 = 8;
pub const DESIGNATOR_TEAMS: u32 = 0x10;
pub const DESIGNATORS: u16 = 9;

/// Holds King of the Hill's globals' address (`E+0xfc` in a KotH game).
pub const KOTH_GLOBALS: u32 = points::KOTH_GLOBALS.base;
/// In them: the hill in play (`i32`, 0 to 7).
pub const KOTH_HILL: u32 = 0x1a0;

/// `scnr+0x118`: the netgame flags block (count, then address), `0x20`
/// each.
pub const NETGAME_FLAGS: u32 = 0x118;
pub const NETGAME_FLAG_STRIDE: u32 = 0x20;
/// In a flag: its facing (`f32`, radians counter-clockwise from +x).
pub const FLAG_FACING: u32 = 0xc;
pub const FLAG_TYPE: u32 = 0x10;
pub const FLAG_TEAM: u32 = 0x12;
/// Netgame flags looked at, at most.
pub const NETGAME_FLAG_LIMIT: i32 = 512;

/// Netgame flag types the engine asks for (`0x243b10`, `0x2bd164`).
pub mod kind {
    pub const CTF_SPAWN: u16 = 0;
    pub const CTF_RETURN: u16 = 1;
    pub const BOMB_SPAWN: u16 = 2;
    pub const BOMB_RETURN: u16 = 3;
    pub const TERRITORY: u16 = 10;
    pub const BALL_SPAWN: u16 = 4;
    /// Hill `n`'s corners are type `HILL + n`, `n` 0 to 7.
    pub const HILL: u16 = 11;
    pub const HILLS: u16 = 8;
}

/// Entries retain both salted carrier handles, including an unowned AI unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registered {
    pub object: DatumHandle,
    /// Native game team (8 is neutral), ball number or territory index.
    pub designator: i16,
    pub unit: Option<DatumHandle>,
    pub player: Option<DatumHandle>,
}

/// Check the span of a retail objective table before deriving addresses
/// from its guest pointer. A transient garbage pointer is a read fault.
pub fn check_span(base: u32, len: u32) -> Result<(), MemFault> {
    if u64::from(base) + u64::from(len) > 1 << 32 {
        return Err(MemFault {
            addr: base,
            len,
            write: false,
        });
    }
    Ok(())
}

pub fn registered(m: &dyn GuestMemory, e: u32) -> Result<Vec<Registered>, MemFault> {
    check_span(e, OBJECTIVES + OBJECTIVE_STRIDE * OBJECTIVE_MAX)?;
    let count = m.u32(e + OBJECTIVE_COUNT)?;
    if count > OBJECTIVE_MAX {
        return Err(MemFault {
            addr: e + OBJECTIVE_COUNT,
            len: 4,
            write: false,
        });
    }
    let mut records = Vec::with_capacity(count as usize);
    for i in 0..count {
        let r = e + OBJECTIVES + i * OBJECTIVE_STRIDE;
        if let Some(object) = DatumHandle::from_raw(m.u32(r + RECORD_OBJECT)?) {
            records.push(Registered {
                object,
                designator: m.u16(r + RECORD_TEAM)? as i16,
                unit: DatumHandle::from_raw(m.u32(r + RECORD_UNIT)?),
                player: DatumHandle::from_raw(m.u32(r + RECORD_PLAYER)?),
            });
        }
    }
    Ok(records)
}

pub const HILL_OCCUPANTS: u32 = 0x1a8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BombState {
    pub arming: bool,
    pub armed: bool,
    pub planted: bool,
}

/// The planted and armed bits and live arming timer of team 0..=8.
pub fn bomb_state(m: &dyn GuestMemory, a: u32, team: u8) -> Result<BombState, MemFault> {
    let team = u32::from(team);
    check_span(a, 0x1a2 + 2 * team)?;
    let flags = m.u8(a + 0x170 + team)?;
    Ok(BombState {
        arming: m.u16(a + 0x1a0 + 2 * team)? as i16 >= 0,
        armed: flags & 8 != 0,
        planted: flags & 2 != 0,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerritoryState {
    pub index: u8,
    pub flag: u16,
    pub owner: Option<(DatumHandle, i16)>,
    pub occupants: u16,
}

/// Scoring ownership and presence, by site (docs/game/bots.md §3.7).
pub fn territories(m: &dyn GuestMemory, t: u32) -> Result<Vec<TerritoryState>, MemFault> {
    check_span(t, 0x116)?;
    let count = (m.u16(t + 0x114)? as i16).clamp(0, 8) as usize;
    let players = guest_layout::view::players::array(m)?;
    let mut occupants = [0u16; 8];
    if let Some(players) = &players {
        for (id, player) in players.live(m, 16)? {
            if player.listed(m)? && player.unit(m)?.is_some() {
                let site = m.u8(t + 0x95 + 8 * u32::from(id.index()))?;
                if let Some(mask) = occupants.get_mut(usize::from(site)) {
                    *mask |= 1 << id.index();
                }
            }
        }
    }
    let mut sites = Vec::with_capacity(count);
    for (i, &occupants) in occupants.iter().take(count).enumerate() {
        let flag = m.u16(t + 0x60 + 2 * i as u32)?;
        let handle = DatumHandle::from_raw(m.u32(t + 0x70 + 4 * i as u32)?);
        let owner = match (handle, &players) {
            (Some(h), Some(array)) => match array.get(m, h)? {
                Some(p) if p.listed(m)? => Some((h, i16::from(p.team(m)?))),
                _ => None,
            },
            _ => None,
        };
        sites.push(TerritoryState {
            index: i as u8,
            flag,
            owner,
            occupants,
        });
    }
    Ok(sites)
}

/// The current hill in engine corner order, following the live flag indices.
pub fn hill_corners(
    m: &dyn GuestMemory,
    k: u32,
    flags: &[([f32; 3], u16, u16)],
) -> Result<Option<(u8, Vec<[f32; 3]>)>, MemFault> {
    check_span(k, KOTH_HILL + 4)?;
    let id = m.i32(k + KOTH_HILL)?;
    let count = m.i32(k + 0x60)?;
    if !(0..i32::from(kind::HILLS)).contains(&id) || !(4..=16).contains(&count) || count % 2 != 0 {
        return Ok(None);
    }
    let mut corners = Vec::with_capacity(count as usize);
    for i in 0..count as u32 {
        let index = m.u32(k + 4 * i)?;
        let Some(&(at, ty, _)) = flags.get(index as usize) else {
            return Ok(None);
        };
        if ty != kind::HILL + id as u16 {
            return Ok(None);
        }
        corners.push(at);
    }
    Ok(Some((id as u8, corners)))
}

/// The loaded scenario’s netgame flags' facings, in placement order, as
/// [`netgame_flags`] lists them: what turns a box zone round its flag.
pub fn netgame_flag_facings(m: &dyn GuestMemory) -> Result<Vec<f32>, MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Vec::new());
    }
    check_span(s, NETGAME_FLAGS + 8)?;
    let n = m.i32(s + NETGAME_FLAGS)?.clamp(0, NETGAME_FLAG_LIMIT) as u32;
    let at = m.u32(s + NETGAME_FLAGS + 4)?;
    check_span(at, n * NETGAME_FLAG_STRIDE)?;
    (0..n)
        .map(|i| m.f32(at + i * NETGAME_FLAG_STRIDE + FLAG_FACING))
        .collect()
}

/// The loaded scenario’s netgame flags, in placement order.
pub fn netgame_flags(m: &dyn GuestMemory) -> Result<Vec<([f32; 3], u16, u16)>, MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Vec::new());
    }
    check_span(s, NETGAME_FLAGS + 8)?;
    let n = m.i32(s + NETGAME_FLAGS)?.clamp(0, NETGAME_FLAG_LIMIT) as u32;
    let at = m.u32(s + NETGAME_FLAGS + 4)?;
    check_span(at, n * NETGAME_FLAG_STRIDE)?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let f = at + i * NETGAME_FLAG_STRIDE;
        out.push((m.vec3(f)?, m.u16(f + FLAG_TYPE)?, m.u16(f + FLAG_TEAM)?));
    }
    Ok(out)
}
