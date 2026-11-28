//! Checkpoints with more than two players (docs/live/live-coop-join.md
//! 13.2).
//!
//! `SAVE_SAFE` (`0x226190`, the body of `game_safe_to_save`) refuses a
//! checkpoint while any player is dead (`players_globals+5`, written by
//! `0x152340`: a player with no unit and no first spawn pending) or any
//! player's unit is unsafe (`0x14df40`, `0x14deb0`, which walk the players'
//! in-use bits). With 16 players a save would rarely come.
//!
//! The rule, with a player of index 2 or more in the game:
//!
//! | who | counts |
//! |---|---|
//! | players 0 and 1 | always, as retail: either dead or unsafe holds the save |
//! | a player 2+ dead, or past [`LEASH`] from the scripts' `player0` (a rider by its vehicle) | left out |
//! | a player 2+ alive within the leash, on foot or riding | near: one vote |
//!
//! The save goes when retail's check passes over players 0 and 1 with
//! every player 2+ left out, and more than half the squad near the anchor
//! (players 0 and 1 and the near ones) is safe. A near player's vote is
//! retail's check over players 0 and 1 and that player alone.
//!
//! A check leaves players out by taking them out of the players' in-use
//! bits (every walk in it, the datum iterator `0x16bc00` included, reads
//! those bits) and recomputing the dead flag over the rest (only ever
//! lowered); both are put back when it returns. [guess] The check has no
//! side effects, so asking it once per near player is safe.
//!
//! With two players nothing changes: retail. The decision reads only the
//! game, so every console makes the same one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;

use super::{FIRST_EXTRA, LEASH, NONE, Player, layout::*, players, script_players, world_position};

/// Pointer-free: the players' globals block (`0x4e8c20`); `+4` "all dead",
/// `+5` "some player dead" (`0x152340`).
pub const PLAYERS_GLOBALS: u32 = 0x004e_8c20;
pub const SOME_DEAD: u32 = 5;

/// Who a checkpoint asks about: `left_out` (players 2+ dead or far) and
/// `near` (players 2+ alive within the leash), by index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Squad {
    pub left_out: Vec<u16>,
    pub near: Vec<u16>,
}

/// The squad for a check; `None` with no player of index 2 or more
/// (retail's check runs alone). The anchor is the scripts' `player0`'s
/// position (its vehicle's when it rides), else `player1`'s; with neither
/// every living player 2+ is near.
pub fn squad<M: GuestMemory + ?Sized>(m: &M, ps: &[Player]) -> Option<Squad> {
    if !ps.iter().any(|p| p.index >= FIRST_EXTRA) {
        return None;
    }
    let at = script_players(ps)
        .iter()
        .find_map(|p| world_position(m, p.unit));
    let mut sq = Squad {
        left_out: Vec::new(),
        near: Vec::new(),
    };
    for p in ps.iter().filter(|p| p.index >= FIRST_EXTRA) {
        let near = p.unit != NONE
            && match (at, world_position(m, p.unit)) {
                (Some(at), Some(pos)) => super::distance(pos, at) <= LEASH,
                (None, _) => true,
                (Some(_), None) => false,
            };
        if near {
            sq.near.push(p.index);
        } else {
            sq.left_out.push(p.index);
        }
    }
    Some(sq)
}

/// More than half the squad near the anchor is safe: `core` players 0 and
/// 1 (safe, or the check would have held), `near` players 2+, `safe` of
/// them safe.
pub fn majority(core: usize, near: usize, safe: usize) -> bool {
    2 * (core + safe) > core + near
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
    if hidden.is_empty() {
        return None;
    }
    let list: Vec<String> = hidden.iter().map(u16::to_string).collect();
    Some(format!(
        "coop-players: checkpoint checks leave out player(s) {} (dead, or past {LEASH} units from the anchor)",
        list.join(", ")
    ))
}

/// The verdict line, when the near players' votes differ from the last
/// logged (`last`: the near and the unsafe players).
pub fn verdict_note(
    last: &Mutex<Option<(Vec<u16>, Vec<u16>)>>,
    near: &[u16],
    unsafe_: &[u16],
    saves: bool,
) -> Option<String> {
    let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
    let key = (near.to_vec(), unsafe_.to_vec());
    if l.as_ref() == Some(&key) {
        return None;
    }
    *l = Some(key);
    let list: Vec<String> = unsafe_.iter().map(u16::to_string).collect();
    Some(format!(
        "coop-players: checkpoint: {} of {} near player(s) of index 2 or more safe{}; the save {}",
        near.len() - unsafe_.len(),
        near.len(),
        if unsafe_.is_empty() {
            String::new()
        } else {
            format!(" (unsafe: {})", list.join(", "))
        },
        if saves {
            "goes on (majority)"
        } else {
            "waits (no majority)"
        }
    ))
}

async fn check_without(cx: &Cx, hidden: &[u16], reentry: &AtomicBool) -> Result<u32, EntryError> {
    let window = cx.write(|m| {
        let ps = players(m);
        hide(m, &ps, hidden)
    })?;
    reentry.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::SAVE_SAFE.base)).await;
    reentry.store(false, Ordering::SeqCst);
    cx.write(|m| restore(m, &window))?;
    Ok(r?.eax)
}

/// `SAVE_SAFE` by the squad's majority: retail's check over players 0 and
/// 1 with every player 2+ left out; when it passes, the same check once
/// per near player with that one back in.
pub async fn safe_to_save(
    cx: Cx,
    squad: Squad,
    reentry: Arc<AtomicBool>,
    last: Arc<Mutex<Option<(Vec<u16>, Vec<u16>)>>>,
) -> Result<Ret, EntryError> {
    let mut all = squad.left_out.clone();
    all.extend(&squad.near);
    let core = check_without(&cx, &all, &reentry).await?;
    if core & 0xff == 0 || squad.near.is_empty() {
        return Ok(Ret::new(core, 0));
    }
    let mut unsafe_ = Vec::new();
    for &p in &squad.near {
        let hidden: Vec<u16> = all.iter().copied().filter(|&h| h != p).collect();
        if check_without(&cx, &hidden, &reentry).await? & 0xff == 0 {
            unsafe_.push(p);
        }
    }
    let core_n = cx.read(|m| players(m).iter().filter(|p| p.index < FIRST_EXTRA).count());
    let saves = majority(core_n, squad.near.len(), squad.near.len() - unsafe_.len());
    if let Some(line) = verdict_note(&last, &squad.near, &unsafe_, saves) {
        eprintln!("{line}");
    }
    Ok(Ret::new((core & !0xff) | u32::from(saves), 0))
}
