//! The engine's objectives as a script on the host sees them
//! (docs/game/rules-engine.md, "The host runtime"): `for each objectives`,
//! `o.carrier` (`carrier_of`) and `reset_objective(o)`.
//!
//! The set and the carrier are the objective registry `E+0xc18`/`E+0xc1c`
//! (`engine::objectives`): each record's object, and the player carrying
//! it. `reset_objective` is made on Oddball alone (`caps` names the
//! carrier): the engine's own ball return, `ODDBALL_BALL_RETURN`
//! (`0x2bc340`), with the record's ball number in EAX, which drops the ball
//! from its carrier and moves it to its spawn flag through the shared
//! objective return `0x15e360`. The object keeps its datum, so the
//! script's object variables stay with it.

use guest_layout::entry::{Cx, EntryError, GuestCall};
use engine::objectives::{self as objs, Registered};
use guest_layout::{GuestMemory, MemFault, points};

/// The engine index whose objectives a host returns.
pub const ODDBALL: i32 = 3;
/// In the Oddball globals: each ball's spawn flag index (`i32`, -1 none).
pub const ODDBALL_SPAWNS: u32 = 0xc;
/// In the game globals: the variant's ball count (`i16`, record `+0xf4`),
/// the bound of the Oddball tick's ball loop.
pub const BALL_COUNT: u32 = 0x230;

fn fault(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

fn read_fault(e: MemFault) -> anyhow::Error {
    anyhow::anyhow!("guest memory at {:#x} faulted", e.addr)
}

/// The registry's records; none without the engine.
pub fn records(m: &dyn GuestMemory) -> Result<Vec<Registered>, MemFault> {
    let e = m.u32(points::GAME_ENGINE.base)?;
    if e == 0 {
        return Ok(Vec::new());
    }
    objs::registered(m, e)
}

/// The objective objects (salted datums), in record order.
pub fn objects(m: &dyn GuestMemory) -> Result<Vec<u32>, MemFault> {
    Ok(records(m)?.iter().map(|r| r.object.raw()).collect())
}

/// The roster slot of the player carrying `object`, or none when it is not
/// carried or not an objective.
pub fn carrier(m: &dyn GuestMemory, object: u32) -> Result<Option<u8>, MemFault> {
    Ok(records(m)?
        .into_iter()
        .find(|r| r.object.raw() == object)
        .and_then(|r| r.player)
        .map(|p| p.index() as u8))
}

/// The ball number `reset_objective(object)` returns, or why it cannot.
pub fn ball_of(m: &dyn GuestMemory, object: u32) -> anyhow::Result<u32> {
    let engine = super::guest::engine_index(m).map_err(read_fault)?;
    anyhow::ensure!(
        engine == Some(ODDBALL),
        "reset_objective: the game is engine {}, and a host returns objectives only on Oddball ({ODDBALL})",
        engine.map_or_else(|| "none".to_string(), |e| e.to_string())
    );
    let record = records(m)
        .map_err(read_fault)?
        .into_iter()
        .find(|r| r.object.raw() == object)
        .ok_or_else(|| anyhow::anyhow!("reset_objective: {object:#010x} is not a registered objective"))?;
    let ball = i32::from(record.designator);
    let g = m.u32(points::GAME_GLOBALS.base).map_err(read_fault)?;
    let count = if g == 0 {
        0
    } else {
        i32::from(m.u16(g + BALL_COUNT).map_err(read_fault)? as i16)
    };
    anyhow::ensure!(
        (0..count).contains(&ball),
        "reset_objective: {object:#010x} is ball {ball}, and the variant has {count} balls"
    );
    let globals = m.u32(points::ODDBALL_GLOBALS.base).map_err(read_fault)?;
    anyhow::ensure!(globals != 0, "reset_objective: the Oddball globals are not set up");
    let spawn = m
        .i32(globals + ODDBALL_SPAWNS + 4 * ball as u32)
        .map_err(read_fault)?;
    anyhow::ensure!(spawn != -1, "reset_objective: ball {ball} has no spawn flag");
    Ok(ball as u32)
}

/// Returns the objective to where it spawned through the engine's own
/// routine.
pub async fn reset(cx: &Cx, object: u32) -> anyhow::Result<()> {
    let ball = cx.read(|m| ball_of(m, object))?;
    cx.call(GuestCall::new(points::ODDBALL_BALL_RETURN.base).eax(ball))
        .await
        .map_err(fault)?;
    Ok(())
}
