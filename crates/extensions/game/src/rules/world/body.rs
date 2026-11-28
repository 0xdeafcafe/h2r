//! `set_body` on the host (docs/game/rules-engine.md, "The engine API";
//! the evidence is docs/game/notes/2026-11-24-siege-spawns-and-bodies.md).
//!
//! The VM keeps each team's body in its state (`rules::vm::RulesExt::bodies`).
//! A multiplayer player's body is the map's player representation that the
//! active configuration's model byte (player `+0x88`, 0 the Spartan, 1 the
//! Elite) picks: `PLAYER_SPAWN` (`0x14f2e0`) asks the representation pick
//! `0x14f040` for the unit type (model 0 is representation 2, model 1 is
//! 3, in a multiplayer game), spawns that unit, and hands it over through
//! `0x14cad0`, which reads the byte again for the local player's
//! representation.
//!
//! For a player whose team has a body, the feature writes the body's model
//! byte before `PLAYER_SPAWN` runs and puts the player's own back after it
//! ([`put`], [`restore`]), so the spawn's every read sees the team's body
//! and nothing else of the player's configuration changes: the lobby, the
//! scoreboard and the next game keep the player's own choice. The spawn is
//! the host's; clients get the unit, whose type is the body's.

use ::rules::bodies::Body;
use guest_layout::{GuestMemory, MemFault};

use crate::firefight::guest as ff;

/// The active configuration's model byte in a player datum (configuration
/// `+0x44`, model `+0x44` within it; docs/game/bots.md, "A phantom's
/// player configuration"), as the representation pick reads it
/// (`0x14f05b`).
pub const PLAYER_MODEL: u32 = 0x88;

/// What [`put`] wrote over: the byte's address and the player's own value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swap {
    pub at: u32,
    pub was: u8,
}

/// Writes `body`'s model byte into player `slot`'s configuration; `None`
/// when it already holds it.
pub fn put(m: &mut dyn GuestMemory, slot: u8, body: Body) -> Result<Option<Swap>, MemFault> {
    let at = ff::player_record(&*m, slot)? + PLAYER_MODEL;
    let was = m.u8(at)?;
    if was == body.model() {
        return Ok(None);
    }
    m.set_u8(at, body.model())?;
    Ok(Some(Swap { at, was }))
}

/// Puts the player's own model byte back.
pub fn restore(m: &mut dyn GuestMemory, swap: Swap) -> Result<(), MemFault> {
    m.set_u8(swap.at, swap.was)
}
