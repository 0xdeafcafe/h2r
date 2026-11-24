//! The rules runtime's guest half (docs/game/rules-engine.md, "The engine
//! API"): the reads behind the script's read entries, and the engine calls
//! behind its actions.
//!
//! Reads take guest memory directly. An action that calls the engine is an
//! `async` body over a [`Cx`] (`engine::entry`), made at a tick boundary.
//! Every call goes to the engine's own routine, named in `points.rs`; the
//! lives and respawn bodies are Firefight's (`crate::firefight::guest`).

use engine::entry::{Cx, EntryError, GuestCall};
use engine::objectives::{OBJECTIVE_COUNT, OBJECTIVE_MAX, OBJECTIVE_STRIDE, OBJECTIVES, RECORD_OBJECT};
use engine::{GuestMemory, MemFault, points};
use guest_layout::game::{PLAYER_STATS, PLAYER_STATS_STRIDE, TEAM_STATS, TEAM_STATS_STRIDE};

use crate::firefight::guest as ff;

/// The active variant record in the game globals (`G+0x13c`,
/// docs/live/live-gamestate.md).
pub const VARIANT: u32 = crate::variant_ext::ACTIVE_VARIANT;
/// In the record: the engine index (`i32`), flags (bit 0 team play), the
/// score limit and the time limit in seconds (`guest_layout::game::GameVariant`).
pub const VARIANT_ENGINE: u32 = 0x44;
pub const VARIANT_FLAGS: u32 = 0x48;
pub const VARIANT_SCORE_LIMIT: u32 = 0x50;
pub const VARIANT_TIME_LIMIT: u32 = 0x54;
/// Variant flags bit 0: team play.
pub const TEAM_PLAY: u32 = guest_layout::game::TEAM_PLAY;
/// The statborg rows' score, kills and deaths (`PlayerStats`, `TeamStats`).
const STATS_SCORE: u32 = 0x0;
const STATS_KILLS: u32 = 0x4;
const STATS_DEATHS: u32 = 0x6;
/// The engine globals' round start tick (`GameEngine::ROUND_START_TICK`,
/// likely).
pub const ROUND_START_TICK: u32 = 0x70;
/// The game-time record: ticks a second (`GameTime::TICK_RATE`).
const TIME_TICK_RATE: u32 = 0x2;
/// What `round_time_left()` answers when the variant has no time limit.
pub const NO_TIME_LIMIT: i32 = i32::MAX;
/// The engine's score clamp (`0x15b849`).
pub const SCORE_CLAMP: i32 = 30_000;

/// The variant fields the runtime reads, and where. A script that reads
/// any other [`::rules::caps::VARIANT_FIELDS`] name is refused at load:
/// its offset is not traced.
pub const VARIANT_FIELDS_READ: &[&str] = &["team_play", "score_limit", "time_limit", "lives"];

fn globals(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    Ok(Some(m.u32(points::GAME_GLOBALS.base)?).filter(|&g| g != 0))
}

fn engine(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    Ok(Some(m.u32(points::GAME_ENGINE.base)?).filter(|&e| e != 0))
}

/// The active record's engine index, or none before the globals exist.
pub fn engine_index(m: &dyn GuestMemory) -> Result<Option<i32>, MemFault> {
    let Some(g) = globals(m)? else {
        return Ok(None);
    };
    Ok(Some(m.i32(g + VARIANT + VARIANT_ENGINE)?))
}

/// The active record's extension block.
pub fn block(m: &dyn GuestMemory) -> Result<Option<[u8; variants::ext::LEN]>, MemFault> {
    let Some(g) = globals(m)? else {
        return Ok(None);
    };
    let mut b = [0u8; variants::ext::LEN];
    m.read(g + crate::variant_ext::BLOCK, &mut b)?;
    Ok(Some(b))
}

/// The options a built-in plays with under a retail record: its type
/// area's settings mapped to the built-in's own options
/// (`variants::portable::retail_options`). `None` for a record on the
/// script index, whose options are the record's script options, and
/// before the globals exist.
pub fn retail_options(m: &dyn GuestMemory, ir: &::rules::Ir) -> Result<Option<u32>, String> {
    let Some(g) = globals(m).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let mut raw = [0u8; variants::record::RECORD_SIZE];
    m.read(g + VARIANT, &mut raw).map_err(|e| e.to_string())?;
    let record = variants::GameVariant::from_bytes(&raw).map_err(|e| e.to_string())?;
    if record.game_type() == variants::record::GameType::Script {
        return Ok(None);
    }
    variants::portable::retail_options(ir, &record)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// A record field by its [`VARIANT_FIELDS_READ`] name; 0 without globals
/// and for a name not listed (the load refused such a script).
pub fn variant(m: &dyn GuestMemory, field: &str) -> Result<i32, MemFault> {
    let Some(g) = globals(m)? else {
        return Ok(0);
    };
    let v = g + VARIANT;
    Ok(match field {
        "team_play" => (m.u32(v + VARIANT_FLAGS)? & TEAM_PLAY != 0) as i32,
        "score_limit" => m.i32(v + VARIANT_SCORE_LIMIT)?,
        "time_limit" => m.i32(v + VARIANT_TIME_LIMIT)?,
        "lives" => m.i32(g + ff::GLOBALS_LIVES)?,
        _ => 0,
    })
}

/// A player's statborg row field, as an `i32`; 0 without the engine.
fn player_stat(m: &dyn GuestMemory, slot: u8, field: u32, signed: bool) -> Result<i32, MemFault> {
    let Some(e) = engine(m)? else {
        return Ok(0);
    };
    let at = e + PLAYER_STATS + u32::from(slot) * PLAYER_STATS_STRIDE + field;
    let v = m.u16(at)?;
    Ok(if signed { v as i16 as i32 } else { v as i32 })
}

/// The player's score (`i16`).
pub fn player_score(m: &dyn GuestMemory, slot: u8) -> Result<i32, MemFault> {
    player_stat(m, slot, STATS_SCORE, true)
}

pub fn kills(m: &dyn GuestMemory, slot: u8) -> Result<i32, MemFault> {
    player_stat(m, slot, STATS_KILLS, false)
}

pub fn deaths(m: &dyn GuestMemory, slot: u8) -> Result<i32, MemFault> {
    player_stat(m, slot, STATS_DEATHS, false)
}

/// The team's score (`i16`); 0 without the engine.
pub fn team_score(m: &dyn GuestMemory, team: u8) -> Result<i32, MemFault> {
    let Some(e) = engine(m)? else {
        return Ok(0);
    };
    let at = e + TEAM_STATS + u32::from(team) * TEAM_STATS_STRIDE + STATS_SCORE;
    Ok(m.u16(at)? as i16 as i32)
}

/// The player's lives remaining (`i16`, -1 unlimited); 0 without a player.
pub fn lives(m: &dyn GuestMemory, slot: u8) -> Result<i32, MemFault> {
    if ff::player_datum(m, slot)?.is_none() {
        return Ok(0);
    }
    let at = ff::player_record(m, slot)?;
    Ok(m.u16(at + ff::PLAYER_LIVES)? as i16 as i32)
}

/// Ticks left in the round: the time limit (seconds, at the game's tick
/// rate) less the ticks since the round started, at least 0;
/// [`NO_TIME_LIMIT`] with no limit.
pub fn round_time_left(m: &dyn GuestMemory) -> Result<i32, MemFault> {
    let limit = variant(m, "time_limit")?;
    if limit <= 0 {
        return Ok(NO_TIME_LIMIT);
    }
    let (Some(e), Some(tick)) = (engine(m)?, bots::guest::game_tick(m)) else {
        return Ok(NO_TIME_LIMIT);
    };
    let time = m.u32(points::GAME_TIME.base)?;
    let rate = if time == 0 {
        ::rules::caps::TICKS_PER_SECOND
    } else {
        i32::from(m.u16(time + TIME_TICK_RATE)? as i16).max(1)
    };
    let start = m.u32(e + ROUND_START_TICK)?;
    let elapsed = i64::from(tick.wrapping_sub(start));
    let left = i64::from(limit) * i64::from(rate) - elapsed;
    Ok(left.clamp(0, i64::from(i32::MAX)) as i32)
}

/// Whether `object` (a datum) is one of the engine's registered objectives
/// (`engine::objectives`).
pub fn is_objective(m: &dyn GuestMemory, object: u32) -> Result<bool, MemFault> {
    let Some(e) = engine(m)? else {
        return Ok(false);
    };
    let count = m.i32(e + OBJECTIVE_COUNT)?.clamp(0, OBJECTIVE_MAX as i32) as u32;
    for i in 0..count {
        if m.u32(e + OBJECTIVES + i * OBJECTIVE_STRIDE + RECORD_OBJECT)? == object {
            return Ok(true);
        }
    }
    Ok(false)
}

fn guest(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

/// Add `delta` to the player's score through `SCORE_ADD`, which takes the
/// change's low 16 bits, signed, in EAX and the salted player datum on the
/// stack, updates the team score and the statborg's replication and checks
/// the score limit. A change outside the engine's clamp is clamped to it.
/// An engine that takes no score is an error.
pub async fn score(cx: &Cx, slot: u8, delta: i32) -> anyhow::Result<()> {
    let delta = delta.clamp(-SCORE_CLAMP, SCORE_CLAMP);
    let datum = cx
        .read(|m| ff::player_datum(m, slot))
        .map_err(guest)?
        .ok_or_else(|| anyhow::anyhow!("no player in slot {slot}"))?;
    let r = cx
        .call(
            GuestCall::new(points::SCORE_ADD.base)
                .eax(delta as u32)
                .args([datum]),
        )
        .await
        .map_err(guest)?;
    anyhow::ensure!(r.eax as u8 != 0, "the engine took no score for slot {slot}");
    Ok(())
}

/// Set the player's lives remaining (player `+0x1ac`, `i16`; -1 is
/// unlimited) and mark the player entity's lives bit for replication
/// (`PLAYER_ENTITY_DIRTY`), as Firefight's respawn does with one life.
pub async fn set_lives(cx: &Cx, slot: u8, n: i32) -> anyhow::Result<()> {
    let n = n.clamp(-1, i32::from(i16::MAX)) as i16;
    let datum = cx
        .read(|m| ff::player_datum(m, slot))
        .map_err(guest)?
        .ok_or_else(|| anyhow::anyhow!("no player in slot {slot}"))?;
    cx.write(|m| {
        let at = ff::player_record(m, slot)?;
        m.set_u16(at + ff::PLAYER_LIVES, n as u16)
    })
    .map_err(guest)?;
    cx.call(GuestCall::new(points::PLAYER_ENTITY_DIRTY.base).args([datum, ff::LIVES_DIRTY]))
        .await
        .map_err(guest)?;
    Ok(())
}

/// Whether this machine's engine keeps scores and lives
/// (`GAME_ENGINE_AUTHORITATIVE`, `0x15b2f0`; AL).
pub async fn authoritative(cx: &Cx) -> anyhow::Result<bool> {
    let r = cx
        .call(GuestCall::new(points::GAME_ENGINE_AUTHORITATIVE.base))
        .await
        .map_err(guest)?;
    Ok(r.eax as u8 != 0)
}
