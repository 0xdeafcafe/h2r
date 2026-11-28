//! Muzzle reports for the Clankers' hearing. The weapon-fire observer
//! supplies only an event position and time, never a live track or target.
//! The shooter's player slot rides along for two uses only: telling a
//! bot's own fire from others', and the motion tracker's firing dots
//! (`guest::radar`), which a player sees on the HUD too.

use std::collections::VecDeque;
use std::sync::Mutex;

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{objects, players};

use crate::brain::Vec3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShotSound {
    pub at: Vec3,
    pub tick: u32,
    /// The player slot whose unit fired, when a player's unit holds the
    /// weapon. The brain compares it with its own slot and nothing else.
    pub shooter: Option<u32>,
}

/// A shot shows its shooter on the motion tracker for this many ticks
/// (`firing`), and a report is heard for as long (`recent_into`).
pub(crate) const HEARD_TICKS: u32 = 6;

static SHOTS: Mutex<VecDeque<ShotSound>> = Mutex::new(VecDeque::new());
const KEPT: usize = 64;

/// Called by the bot feature's weapon-fire observer.
pub fn observe_shot(m: &dyn GuestMemory, weapon: u32) {
    if let Ok(Some(shot)) = shot_from_guest(m, weapon) {
        let mut shots = SHOTS.lock().unwrap_or_else(|e| e.into_inner());
        if shots.len() >= KEPT {
            shots.pop_front();
        }
        shots.push_back(shot);
    }
}

pub fn clear_shots() {
    SHOTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub(crate) fn recent_into(tick: u32, out: &mut Vec<ShotSound>) {
    out.clear();
    let shots = SHOTS.lock().unwrap_or_else(|e| e.into_inner());
    out.extend(
        shots
            .iter()
            .filter(|s| tick.wrapping_sub(s.tick) <= HEARD_TICKS)
            .copied(),
    );
}

/// The player slots that fired within [`HEARD_TICKS`] of `tick`, as a
/// mask (`perceive::player_bit`): the motion tracker shows a firing player
/// whether it moves or not (docs/game/bots.md, "The motion tracker").
pub(crate) fn firing(tick: u32) -> u16 {
    let shots = SHOTS.lock().unwrap_or_else(|e| e.into_inner());
    firing_in(shots.iter(), tick)
}

fn firing_in<'a>(shots: impl Iterator<Item = &'a ShotSound>, tick: u32) -> u16 {
    shots
        .filter(|s| tick.wrapping_sub(s.tick) <= HEARD_TICKS)
        .filter_map(|s| s.shooter)
        .fold(0, |mask, slot| mask | crate::perceive::player_bit(slot))
}

fn shot_from_guest(m: &dyn GuestMemory, weapon: u32) -> Result<Option<ShotSound>, MemFault> {
    let (Some(tick), Some(weapon)) = (crate::guest::game_tick(m), DatumHandle::from_raw(weapon))
    else {
        return Ok(None);
    };
    let Some((objects::WEAPON, weapon)) = objects::get(m, weapon)? else {
        return Ok(None);
    };
    // An attached weapon can carry a stale origin. Use the holder's place
    // when available, including mounted guns, a little above its feet.
    let mut at = weapon.position(m)?;
    let mut shooter = None;
    if let Some(parent) = weapon.parent(m)?
        && let Some((_, holder)) = objects::get(m, parent)?
    {
        at = holder.position(m)?;
        at[2] += crate::mind::EYE;
        // A fault reading the players only loses the slot, not the sound.
        shooter = shooter_slot(m, parent).ok().flatten();
    }
    Ok(at
        .iter()
        .all(|v| v.is_finite())
        .then_some(ShotSound { at, tick, shooter }))
}

/// The slot of the player whose unit is `holder`, if any.
fn shooter_slot(m: &dyn GuestMemory, holder: DatumHandle) -> Result<Option<u32>, MemFault> {
    let Some(array) = players::array(m)? else {
        return Ok(None);
    };
    for player in array.live_iter(m, 16)? {
        let (handle, p) = player?;
        if p.unit(m)?.is_some_and(|unit| unit.raw() == holder.raw()) {
            return Ok(Some(u32::from(handle.index())));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
