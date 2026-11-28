//! The scores at each round's start, as the rules runtime reads them
//! (docs/runtime/sim.md, "Rounds"): every player's `player_score` and
//! each team's score, read from the game's statborg at the start of the
//! script's round (`on round_start`, before the handler's own scoring).
//! A sim report's `rounds` carries the log; the runtime clears it when a
//! script starts.

use std::sync::{Mutex, MutexGuard};

/// The most rounds kept (a script's `round_count` is far below it).
const MAX: usize = 256;

/// One round's start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The script's tick at the round start.
    pub tick: u32,
    /// `(slot, team, player_score)` of every player in the game.
    pub players: Vec<(u8, u8, i32)>,
    /// `(team, score)` of every team a player is on.
    pub teams: Vec<(u8, i32)>,
}

static LOG: Mutex<Vec<Snapshot>> = Mutex::new(Vec::new());

fn log() -> MutexGuard<'static, Vec<Snapshot>> {
    LOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Forgets the log: a script started.
pub fn clear() {
    log().clear();
}

/// Adds a round's start.
pub fn push(s: Snapshot) {
    let mut l = log();
    if l.len() < MAX {
        l.push(s);
    }
}

/// The log, in round order; it is left empty.
pub fn take() -> Vec<Snapshot> {
    std::mem::take(&mut *log())
}

/// The scores the game holds now, read through the same guest reads a
/// script's `player_score` and team score make.
#[cfg(feature = "rules")]
pub fn read(m: &dyn guest_layout::GuestMemory, tick: u32) -> Snapshot {
    use crate::rules::guest;
    let players = crate::firefight::runtime::read_players(m).unwrap_or_default();
    let mut rows: Vec<(u8, u8, i32)> = players
        .iter()
        .map(|(p, _)| (p.slot, p.team, guest::player_score(m, p.slot).unwrap_or(0)))
        .collect();
    rows.sort_unstable();
    let mut teams: Vec<u8> = rows.iter().map(|r| r.1).collect();
    teams.dedup_by_key(|t| *t);
    teams.sort_unstable();
    teams.dedup();
    Snapshot {
        tick,
        players: rows,
        teams: teams
            .into_iter()
            .map(|t| (t, guest::team_score(m, t).unwrap_or(0)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_rounds_in_order_and_empties_on_take() {
        clear();
        push(Snapshot { tick: 0, players: vec![(0, 0, 0)], teams: vec![(0, 0)] });
        push(Snapshot { tick: 900, players: vec![(0, 0, 3)], teams: vec![(0, 5)] });
        let got = take();
        assert_eq!(got.iter().map(|s| s.tick).collect::<Vec<_>>(), [0, 900]);
        assert_eq!(got[1].players, [(0, 0, 3)]);
        assert!(take().is_empty());
    }
}
