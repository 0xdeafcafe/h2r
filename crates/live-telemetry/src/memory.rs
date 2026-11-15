use std::collections::BTreeSet;

use live_proto::game::{Player, TeamScore};
use runtime::{Guest, GuestFault};

use crate::event::{GameInfo, ObjectState, PlayerState, PlayerStats, Snapshot};
use crate::layout;

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

fn wide_text(guest: &Guest, address: u32, code_units: usize) -> ReadResult<String> {
    let mut values = Vec::with_capacity(code_units);
    for i in 0..code_units {
        let c = guest.u16(address + 2 * i as u32)?;
        if c == 0 {
            break;
        }
        values.push(c);
    }
    Ok(String::from_utf16_lossy(&values))
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
    Ok(GameInfo {
        map: text(guest, layout::CACHE_MAP_NAME, 32)?,
        mode: mode.into(),
        variant: variant_name,
        team_play: guest.u32(variant + layout::VARIANT_FLAGS)? & 1 != 0,
        score_limit: nonzero(i32(guest.u32(variant + layout::VARIANT_SCORE_LIMIT)?)),
        time_limit_s: nonzero(i32(guest.u32(variant + layout::VARIANT_TIME_LIMIT)?)),
        tick_rate: guest.u16(options + layout::OPTIONS_TICK_RATE)?.max(1),
        tick: u64::from(guest.u32(time + layout::TIME_TICK)?),
    })
}

pub fn players(guest: &Guest) -> ReadResult<Vec<PlayerState>> {
    let array = ptr(guest, layout::PLAYERS_PTR)?;
    let data = ptr(guest, array + layout::DATUM_DATA)?;
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    let statborg = engine + layout::ENGINE_STATBORG;
    let has_stats = guest.u32(statborg)? != 0;
    let high_water = guest.u32(array + layout::DATUM_HIGH_WATER)?.min(16);
    let mut out = Vec::new();
    for slot in 0..high_water {
        let address = data + slot * layout::PLAYER_SIZE;
        let salt = guest.u16(address + layout::PLAYER_SALT)?;
        if salt == 0 || guest.u16(address + layout::PLAYER_FLAGS)? & 2 != 0 {
            continue;
        }
        let stats = if has_stats {
            let row = engine + layout::ENGINE_PLAYER_STATS + slot * layout::PLAYER_STAT_SIZE;
            PlayerStats {
                score: i64::from(i16(guest.u16(row + layout::STAT_SCORE)?)),
                kills: u32::from(guest.u16(row + layout::STAT_KILLS)?),
                deaths: u32::from(guest.u16(row + layout::STAT_DEATHS)?),
                assists: u32::from(guest.u16(row + layout::STAT_ASSISTS)?),
                suicides: u32::from(guest.u16(row + layout::STAT_SUICIDES)?),
            }
        } else {
            PlayerStats::ZERO
        };
        out.push(PlayerState {
            datum: (u32::from(salt) << 16) | slot,
            player: Player {
                index: slot,
                name: wide_text(
                    guest,
                    address + layout::PLAYER_NAME,
                    layout::PLAYER_NAME_CODE_UNITS,
                )?,
                xuid: None,
                team: i32::from(guest.u8(address + layout::PLAYER_TEAM)? as i8),
                local: Some(i16(guest.u16(address + layout::PLAYER_LOCAL_USER)?) != -1),
            },
            unit: guest.u32(address + layout::PLAYER_UNIT)?,
            stats,
        });
    }
    Ok(out)
}

pub fn team_scores(guest: &Guest, players: &[PlayerState]) -> ReadResult<Vec<TeamScore>> {
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    let statborg = engine + layout::ENGINE_STATBORG;
    if guest.u32(statborg)? == 0 {
        return Ok(Vec::new());
    }
    let teams: BTreeSet<i32> = players
        .iter()
        .map(|p| p.player.team)
        .filter(|t| (0..8).contains(t))
        .collect();
    teams
        .into_iter()
        .map(|team| {
            let row = engine + layout::ENGINE_TEAM_STATS + team as u32 * layout::TEAM_STAT_SIZE;
            Ok(TeamScore {
                team,
                score: i64::from(i16(guest.u16(row + layout::STAT_SCORE)?)),
            })
        })
        .collect()
}

pub fn object(guest: &Guest, datum: u32) -> ReadResult<Option<ObjectState>> {
    fn transform(guest: &Guest, body: u32) -> ReadResult<([f64; 3], [f64; 3])> {
        let mut pos = [0.0; 3];
        let mut forward = [0.0; 3];
        for (i, value) in pos.iter_mut().enumerate() {
            *value = f64::from(guest.f32(body + layout::BODY_POSITION + 4 * i as u32)?);
        }
        for (i, value) in forward.iter_mut().enumerate() {
            *value = f64::from(guest.f32(body + layout::BODY_FORWARD + 4 * i as u32)?);
        }
        Ok((pos, forward))
    }

    if datum == u32::MAX {
        return Ok(None);
    }
    let array = ptr(guest, layout::OBJECTS_PTR)?;
    let slot = datum & 0xffff;
    if slot >= guest.u32(array + layout::DATUM_HIGH_WATER)? {
        return Ok(None);
    }
    let data = ptr(guest, array + layout::DATUM_DATA)?;
    let header = data + slot * layout::OBJECT_HEADER_SIZE;
    let salt = guest.u16(header + layout::OBJECT_SALT)?;
    if salt == 0 || u32::from(salt) != datum >> 16 {
        return Ok(None);
    }
    if guest.u8(header + layout::OBJECT_TYPE)? > 1 {
        return Ok(None);
    }
    let body = guest.u32(header + layout::OBJECT_BODY)?;
    if body == 0 {
        return Ok(None);
    }
    let (mut pos, mut forward) = transform(guest, body)?;
    let parent = guest.u32(body + layout::BODY_PARENT)?;
    let vehicle =
        if parent != u32::MAX && (parent & 0xffff) < guest.u32(array + layout::DATUM_HIGH_WATER)? {
            let parent_header = data + (parent & 0xffff) * layout::OBJECT_HEADER_SIZE;
            let parent_salt = guest.u16(parent_header + layout::OBJECT_SALT)?;
            if parent_salt != 0
                && u32::from(parent_salt) == parent >> 16
                && guest.u8(parent_header + layout::OBJECT_TYPE)? == 1
            {
                let parent_body = guest.u32(parent_header + layout::OBJECT_BODY)?;
                if parent_body == 0 {
                    return Ok(None);
                }
                (pos, forward) = transform(guest, parent_body)?;
                Some(parent)
            } else {
                None
            }
        } else {
            None
        };
    if !pos.iter().chain(forward.iter()).all(|v| v.is_finite()) {
        return Ok(None);
    }
    Ok(Some(ObjectState {
        pos,
        forward,
        vehicle,
    }))
}

pub fn snapshot(guest: &Guest) -> ReadResult<Snapshot> {
    let players = players(guest)?;
    let teams = team_scores(guest, &players)?;
    let objects = players
        .iter()
        .map(|p| object(guest, p.unit))
        .collect::<ReadResult<Vec<_>>>()?;
    let engine = ptr(guest, layout::ENGINE_GLOBALS_PTR)?;
    Ok(Snapshot {
        tick: game_tick(guest)?,
        has_stats: guest.u32(engine + layout::ENGINE_STATBORG)? != 0,
        players,
        teams,
        objects,
    })
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

    pub fn put_object(guest: &mut Guest, slot: u32, parent: u32) {
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest.set_u16(header, 0x9001 + slot as u16).unwrap();
        guest
            .set_u8(header + super::layout::OBJECT_TYPE, 0)
            .unwrap();
        guest
            .set_u32(header + super::layout::OBJECT_BODY, BODY + slot * 0x1000)
            .unwrap();
        let body = BODY + slot * 0x1000;
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
            .set_u32(OBJECTS + super::layout::DATUM_HIGH_WATER, (slot + 1).max(1))
            .unwrap();
    }

    pub fn put_vehicle(guest: &mut Guest, slot: u32) {
        put_object(guest, slot, u32::MAX);
        let header = OBJECT_DATA + slot * super::layout::OBJECT_HEADER_SIZE;
        guest
            .set_u8(header + super::layout::OBJECT_TYPE, 1)
            .unwrap();
        let body = BODY + slot * 0x1000;
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

    pub fn object_datum(slot: u32) -> u32 {
        ((0x9001 + slot) << 16) | slot
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

        let info = game_info(&guest).unwrap();
        assert_eq!(info.map, "lockout");
        assert_eq!(info.mode, "multiplayer");
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
}
