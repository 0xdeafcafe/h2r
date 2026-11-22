//! Muzzle reports for the Clankers' hearing. The weapon-fire observer
//! supplies only an event position and time, never a live track or target.

use std::collections::VecDeque;
use std::sync::Mutex;

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::view::objects;

use crate::brain::Vec3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShotSound {
    pub at: Vec3,
    pub tick: u32,
}

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
            .filter(|s| tick.wrapping_sub(s.tick) <= 6)
            .copied(),
    );
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
    if let Some(parent) = weapon.parent(m)?
        && let Some((_, holder)) = objects::get(m, parent)?
    {
        at = holder.position(m)?;
        at[2] += crate::mind::EYE;
    }
    Ok(at
        .iter()
        .all(|v| v.is_finite())
        .then_some(ShotSound { at, tick }))
}

#[cfg(test)]
mod tests;
