//! `role_traits(p, on)` on the host (docs/game/rules-engine.md, "The host
//! runtime"): the carrier engine's role, and the traits the variant's type
//! area gives it.
//!
//! A host makes it on Juggernaut alone (`caps` names the carrier): the call
//! is the engine's own role setter, `JUGGERNAUT_ROLE_SET` (`0x192970`),
//! with the player's salted datum in EAX and `on` in ECX. It sets or clears
//! the player's bit of the role mask (`[JUGGERNAUT_GLOBALS]+0`), announces
//! a new Juggernaut, marks the engine globals for replication and rebuilds
//! the teams' mask; Juggernaut's trait query (`0x1926c0`, slot `+0x8c`)
//! then answers the role's traits (camo, damage, resistance from the type
//! area) from the mask, here and on every guest. A player already in the
//! asked state is left alone, so the announcement is not made twice.
//!
//! In a script game the engine neither moves nor draws the role
//! ([`super::scoring`]): the script's calls are the only ones.

use engine::entry::{Cx, EntryError, GuestCall};
use engine::{GuestMemory, MemFault, points};

use crate::firefight::guest as ff;

/// The engine index whose role a host sets.
pub const JUGGERNAUT: i32 = 7;
/// In the Juggernaut globals: the role mask, a bit per player index (`u16`).
pub const ROLE_MASK: u32 = 0;

fn fault(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

/// The Juggernaut role mask, or none before the Juggernaut globals exist.
pub fn role_mask(m: &dyn GuestMemory) -> Result<Option<u16>, MemFault> {
    let g = m.u32(points::JUGGERNAUT_GLOBALS.base)?;
    if g == 0 {
        return Ok(None);
    }
    Ok(Some(m.u16(g + ROLE_MASK)?))
}

/// What a `role_traits` call does, read before it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Call the role setter with this salted player datum.
    Set(u32),
    /// The player is already in the asked state.
    Already,
}

/// Reads what `role_traits(slot, on)` does, or why it cannot.
pub fn plan(m: &dyn GuestMemory, slot: u8, on: bool) -> anyhow::Result<Plan> {
    let read = |e: MemFault| anyhow::anyhow!("guest memory at {:#x} faulted", e.addr);
    let engine = super::guest::engine_index(m).map_err(read)?;
    anyhow::ensure!(
        engine == Some(JUGGERNAUT),
        "role_traits: the game is engine {}, and a host sets a role only on Juggernaut ({JUGGERNAUT})",
        engine.map_or_else(|| "none".to_string(), |e| e.to_string())
    );
    let mask = role_mask(m)
        .map_err(read)?
        .ok_or_else(|| anyhow::anyhow!("role_traits: the Juggernaut globals are not set up"))?;
    let datum = ff::player_datum(m, slot)
        .map_err(read)?
        .ok_or_else(|| anyhow::anyhow!("role_traits: no player in slot {slot}"))?;
    // `player_datum` has no player past slot 15, the mask's last bit.
    let has = mask & (1 << slot) != 0;
    Ok(if has == on { Plan::Already } else { Plan::Set(datum) })
}

/// Gives (`on`) or takes the player's role through the engine's setter.
pub async fn set(cx: &Cx, slot: u8, on: bool) -> anyhow::Result<()> {
    match cx.read(|m| plan(m, slot, on))? {
        Plan::Already => Ok(()),
        Plan::Set(datum) => {
            cx.call(
                GuestCall::new(points::JUGGERNAUT_ROLE_SET.base)
                    .eax(datum)
                    .ecx(u32::from(on)),
            )
            .await
            .map_err(fault)?;
            Ok(())
        }
    }
}
