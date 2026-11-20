//! Checkpoints with more than two players (docs/live/live-coop.md 13.5).
//!
//! `SAVE_SAFE` (`0x226190`, the body of `game_safe_to_save`) refuses a
//! checkpoint while any player is dead (`players_globals+5`, written by
//! `0x152340`: a player with no unit and no first spawn pending) or any
//! player's unit is unsafe (`0x14df40`, `0x14deb0`, which walk the players'
//! in-use bits). With 16 players a save would rarely come.
//!
//! The rule: players 0 and 1 always count; a player of index 2 or more
//! counts only while alive and within [`LEASH`] of the anchor (player 0 on
//! foot, else player 1). For the length of the check, the players that do
//! not count are taken out of the players' in-use bits (every walk in it,
//! the datum iterator `0x16bc00` included, reads those bits) and the dead
//! flag is recomputed over the players that do count; both are put back
//! when the check returns. The flag is only ever lowered.
//!
//! Nothing is hidden when every player counts, so two-player co-op is
//! exactly retail. The decision reads only the game, so every console makes
//! the same one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;

use super::{FIRST_EXTRA, LEASH, NONE, Player, anchor, layout::*, object, players};

/// Pointer-free: the players' globals block (`0x4e8c20`); `+4` "all dead",
/// `+5` "some player dead" (`0x152340`).
pub const PLAYERS_GLOBALS: u32 = 0x004e_8c20;
pub const SOME_DEAD: u32 = 5;

/// The players of index 2 or more that do not count against a save: dead
/// (no unit), or alive on foot farther than [`LEASH`] from the anchor. A
/// player 2+ in a vehicle, or any player 2+ when there is no anchor,
/// counts (its distance is not known): as retail.
pub fn not_counted<M: GuestMemory + ?Sized>(m: &M, ps: &[Player]) -> Vec<u16> {
    let at = anchor(m, ps).map(|a| a.1);
    ps.iter()
        .filter(|p| p.index >= FIRST_EXTRA)
        .filter(|p| {
            if p.unit == NONE {
                return true;
            }
            let (Some(at), Some(o)) = (at, object(m, p.unit)) else {
                return false;
            };
            o.parent == NONE && super::distance(o.position, at) > LEASH
        })
        .map(|p| p.index)
        .collect()
}

/// `players_globals+5` over the players that count: some counted player
/// has no unit and no first spawn pending. Never above `old`.
pub fn some_dead_counted(ps: &[Player], hidden: &[u16], old: u8) -> u8 {
    let dead = ps
        .iter()
        .any(|p| !hidden.contains(&p.index) && p.unit == NONE && p.flags & FIRST_SPAWN == 0);
    u8::from(old != 0 && dead)
}

/// What the window changed, to put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// (address, old word) of each in-use word changed.
    pub words: Vec<(u32, u32)>,
    pub some_dead: u8,
}

/// Takes `hidden` out of the in-use bits and lowers the dead flag.
pub fn hide(m: &mut dyn GuestMemory, ps: &[Player], hidden: &[u16]) -> Result<Hidden, MemFault> {
    let hdr = m.u32(points::PLAYERS.base)?;
    let bits = m.u32(hdr.wrapping_add(ARRAY_IN_USE))?;
    let mut words: Vec<(u32, u32)> = Vec::new();
    for &i in hidden {
        let at = bits.wrapping_add(4 * (u32::from(i) >> 5));
        if !words.iter().any(|(a, _)| *a == at) {
            words.push((at, m.u32(at)?));
        }
        let w = m.u32(at)?;
        m.set_u32(at, w & !(1 << (u32::from(i) & 31)))?;
    }
    let g = m.u32(PLAYERS_GLOBALS)?;
    let old = m.u8(g.wrapping_add(SOME_DEAD))?;
    m.set_u8(
        g.wrapping_add(SOME_DEAD),
        some_dead_counted(ps, hidden, old),
    )?;
    Ok(Hidden {
        words,
        some_dead: old,
    })
}

/// Puts back what [`hide`] changed.
pub fn restore(m: &mut dyn GuestMemory, h: &Hidden) -> Result<(), MemFault> {
    for &(at, w) in &h.words {
        m.set_u32(at, w)?;
    }
    let g = m.u32(PLAYERS_GLOBALS)?;
    m.set_u8(g.wrapping_add(SOME_DEAD), h.some_dead)
}

/// The log line for a check without `hidden`, when it differs from the
/// last one logged (`last`): at most one line per change of who is left
/// out.
pub fn note(last: &Mutex<Option<Vec<u16>>>, hidden: &[u16]) -> Option<String> {
    let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
    if l.as_deref() == Some(hidden) {
        return None;
    }
    *l = Some(hidden.to_vec());
    let list: Vec<String> = hidden.iter().map(u16::to_string).collect();
    Some(format!(
        "coop-players: checkpoint checks leave out player(s) {} (dead, or past {LEASH} units from the anchor)",
        list.join(", ")
    ))
}

/// `SAVE_SAFE` with players left out: hide, the original, put back.
pub async fn safe_to_save(
    cx: Cx,
    hidden: Vec<u16>,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    let window = cx.write(|m| {
        let ps = players(m);
        hide(m, &ps, &hidden)
    })?;
    reentry.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::SAVE_SAFE.base)).await;
    reentry.store(false, Ordering::SeqCst);
    cx.write(|m| restore(m, &window))?;
    Ok(Ret::new(r?.eax, 0))
}
