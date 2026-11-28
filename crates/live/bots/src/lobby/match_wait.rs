//! Why a matchmaking lobby this console hosts has not started its game
//! (docs/game/bots.md §8.4, "The matchmaking host's lobby"): what the
//! host's matchmaking update reads, read the same way, so a Clankermatch
//! that sits in its lobby says in one line which check holds it.
//!
//! The host update (`0x71730`) waits for the target session's members to
//! be the matchmaking roster (`+0x4a00`, compared at `0x71982`), then for
//! every peer's map (`0x6e0f0`) and arbitration status (`0x72140`); only a
//! full, map-ready lobby makes an arbitration round (`0x71ac2`) and
//! registers it. The matchmaking start (`0x66050`) wants every peer's
//! status for the lobby's hopper to be 1. A countdown set
//! (`+0x49a8`) is no wait.
use guest_layout::memory::GuestMemory;

use super::{COUNTDOWN, NETWORKING, SESSION_TYPE, TARGET_SESSION, matchmaking_host};
use crate::guest::R;
use crate::phantom::session::at;

/// Target session `+0x49c8`: the lobby's hopper.
pub const HOPPER: u32 = 0x49c8;
/// Target session `+0x49e8`: an arbitration round is set (`0x5c1c0`).
pub const ARBITRATION_ROUND: u32 = 0x49e8;
/// Target session `+0x49fd`: the matchmaking roster is present.
pub const HAS_ROSTER: u32 = 0x49fd;
/// Target session `+0x4a00`: the roster (its parties' count first, its
/// players' count at `+0x184`, their 12-byte ids from `+0x188`).
pub const ROSTER: u32 = 0x4a00;
pub const ROSTER_PLAYERS: u32 = ROSTER + 0x184;
pub const ROSTER_IDS: u32 = ROSTER + 0x188;
/// A peer's arbitration status (`0x73b10`, peer `+0xec`): 1 registering,
/// 2 registered, 5 the registration failed.
pub const PEER_ARBITRATION: u32 = 0xec;
/// The matchmaking state object's result (`0x527500 + 0x978`): 3 active.
pub const MATCHMAKING_RESULT: u32 = 0x0052_7500 + 0x978;
/// The arbitration state (`0x510518`): 2 once registered.
pub const ARBITRATION_STATE: u32 = 0x0051_0518;

const MAX: u32 = 16;
const ID_SIZE: usize = 12;

/// One peer of the target session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub phantom: bool,
    /// 1 failed, 2 loading, 3 and 4 loaded.
    pub map: u32,
    /// The status for the lobby's hopper; 1 is ready.
    pub hopper: u32,
    pub arbitration: u32,
}

/// What [`read`] found in the matchmaking lobby this console hosts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchWait {
    pub players: u32,
    pub limit: u32,
    /// The roster's players, and how many of them are session members.
    pub roster: Option<(u32, u32)>,
    pub peers: Vec<Peer>,
    pub round: bool,
    pub result: Option<u32>,
    pub arbitration_state: Option<u32>,
}

/// The wait in the matchmaking lobby this console hosts. `Ok(None)` when
/// there is nothing to wait for: the game runs (life cycle 8 or 9), its
/// countdown runs, or networking is off. `Err` names a lobby that is not
/// this console's matchmaking lobby, which is a wait of its own.
pub fn read(m: &dyn GuestMemory) -> R<Result<Option<MatchWait>, String>> {
    if m.u8(NETWORKING)? == 0 {
        return Ok(Ok(None));
    }
    let ty = m.u32(SESSION_TYPE)?;
    if matches!(ty, 8 | 9) {
        return Ok(Ok(None));
    }
    let f = super::membership(m)?;
    if !matchmaking_host(&f) {
        return Ok(Err(format!(
            "this console hosts no matchmaking lobby (life cycle {ty}, match session state {}, {})",
            f.target_state
                .map_or_else(|| "none".to_owned(), |s| s.to_string()),
            if f.target_host {
                "this console its host"
            } else {
                "another console its host"
            }
        )));
    }
    let t = m.u32(TARGET_SESSION)?;
    if m.u8(t + COUNTDOWN)? != 0 {
        return Ok(Ok(None));
    }
    let hopper = m.u32(t + HOPPER)?;
    let peer_count = m.i32(t + at::PEER_COUNT)?.clamp(0, MAX as i32) as u32;
    let mut peers = Vec::with_capacity(peer_count as usize);
    for i in 0..peer_count {
        let p = t + at::PEERS + i * at::PEER_SIZE;
        let mut mac = [0; 6];
        m.read(p + at::PEER_MAC, &mut mac)?;
        peers.push(Peer {
            phantom: crate::phantom::handle_of(mac).is_some(),
            map: m.u32(p + at::PEER_MAP_STATUS)?,
            hopper: if hopper < MAX {
                m.u32(p + at::PEER_HOPPERS + hopper * 4)?
            } else {
                0
            },
            arbitration: m.u32(p + PEER_ARBITRATION)?,
        });
    }
    let roster = if m.u8(t + HAS_ROSTER)? != 0 {
        let n = m.u32(t + ROSTER_PLAYERS)?;
        (n <= MAX)
            .then(|| roster_present(m, t, n).map(|q| (n, q)))
            .transpose()?
    } else {
        None
    };
    Ok(Ok(Some(MatchWait {
        players: m.u32(t + at::PLAYER_COUNT)?,
        limit: m.u32(t + at::MAX_PLAYERS)?,
        roster,
        peers,
        round: m.u8(t + ARBITRATION_ROUND)? != 0,
        result: m.u32(MATCHMAKING_RESULT).ok(),
        arbitration_state: m.u32(ARBITRATION_STATE).ok(),
    })))
}

/// How many of the roster's `count` ids are session members (`0x719c5`).
fn roster_present(m: &dyn GuestMemory, s: u32, count: u32) -> R<u32> {
    let mask = m.u32(s + at::PLAYER_MASK)?;
    let mut members = Vec::new();
    for slot in (0..MAX).filter(|i| mask & (1 << i) != 0) {
        let mut id = [0; ID_SIZE];
        m.read(s + at::PLAYERS + slot * at::PLAYER_SIZE, &mut id)?;
        members.push(id);
    }
    let mut present = 0;
    for i in 0..count {
        let mut id = [0; ID_SIZE];
        m.read(s + ROSTER_IDS + i * ID_SIZE as u32, &mut id)?;
        if members.contains(&id) {
            present += 1;
        }
    }
    Ok(present)
}

impl MatchWait {
    /// The first check that holds the lobby, in the host update's order.
    pub fn reason(&self) -> String {
        if self.players < self.limit {
            return format!(
                "the match session has {} of {} players; the rest have not joined \
                 (each bot's \"bots: phantom ...\" line says why it waits)",
                self.players, self.limit
            );
        }
        if let Some((listed, present)) = self.roster
            && present < listed
        {
            return format!(
                "the matchmaking roster lists {listed} players and {present} of them are in the \
                 session (0x71982 waits for the rest)"
            );
        }
        if let Some((i, p)) = self.peers.iter().enumerate().find(|(_, p)| p.hopper != 1) {
            return format!(
                "peer {i}{}'s status for the hopper is {}, not 1 (0x66050)",
                if p.phantom { " (a bot)" } else { "" },
                p.hopper
            );
        }
        if let Some((i, p)) = self
            .peers
            .iter()
            .enumerate()
            .find(|(_, p)| !matches!(p.map, 3 | 4))
        {
            return format!(
                "peer {i}{} has not loaded the map (map status {}: 1 failed, 2 loading; 0x6e0f0)",
                if p.phantom { " (a bot)" } else { "" },
                p.map
            );
        }
        if !self.round {
            return "the lobby is full and loaded but has no arbitration round (0x71ac2 not reached)"
                .to_owned();
        }
        if let Some((i, p)) = self
            .peers
            .iter()
            .enumerate()
            .find(|(_, p)| p.arbitration > 2)
        {
            return format!(
                "peer {i}{}'s arbitration registration failed (status {}; see the \
                 \"live: arbitration ... failed\" line)",
                if p.phantom { " (a bot)" } else { "" },
                p.arbitration
            );
        }
        if self.peers.iter().any(|p| p.arbitration != 2) {
            return "arbitration: still registering the round with Live (peer status 1 until it answers)"
                .to_owned();
        }
        "every traced check reads ready (untraced: see live.matchmaking.trace)".to_owned()
    }

    /// What was read, for the log line's tail.
    pub fn facts(&self) -> String {
        let list = |f: fn(&Peer) -> u32| {
            self.peers
                .iter()
                .map(|p| f(p).to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let shown = |v: Option<u32>| v.map_or_else(|| "?".to_owned(), |v| format!("{v:#x}"));
        format!(
            "players {}/{}, roster {}, peers {} (bots {}), maps [{}], hopper statuses [{}], \
             arbitration [{}], round {}, matchmaking result {}, arbitration state {}",
            self.players,
            self.limit,
            self.roster
                .map_or_else(|| "none".to_owned(), |(n, q)| format!("{q}/{n} present")),
            self.peers.len(),
            self.peers.iter().filter(|p| p.phantom).count(),
            list(|p| p.map),
            list(|p| p.hopper),
            list(|p| p.arbitration),
            if self.round { "set" } else { "none" },
            shown(self.result),
            shown(self.arbitration_state),
        )
    }

    /// The whole line: the reason, then the facts.
    pub fn line(&self) -> String {
        format!("{} [{}]", self.reason(), self.facts())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HOST_PEER, LOBBY_ON, PEER, SESSION, STATE};
    use super::*;
    use guest_layout::testing::FakeGuest;

    const S: u32 = 0x0070_0000;
    const T: u32 = 0x0072_0000;
    const GLOBALS: u32 = 0x0051_0000;
    const MM: u32 = 0x0052_7000;

    /// A full Clanker Doubles lobby this console hosts: the host (peer 0)
    /// and three bots, every map loaded, every hopper status 1, the roster
    /// all present, a round registered.
    fn full_doubles() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(GLOBALS, 0x1000).map(MM, 0x1000);
        g.put_u8(LOBBY_ON, 1)
            .put_u8(NETWORKING, 1)
            .put_u32(SESSION_TYPE, 6)
            .put_u32(SESSION, S)
            .map(S, 0x7500)
            .put_u32(S + STATE, 5)
            .put_u32(TARGET_SESSION, T)
            .map(T, 0x7500)
            .put_u32(T + STATE, 5)
            .put_u32(T + HOST_PEER, 0)
            .put_u32(T + PEER, 0)
            .put_u32(T + HOPPER, 10)
            .put_u32(T + at::PLAYER_COUNT, 4)
            .put_u32(T + at::MAX_PLAYERS, 4)
            .put_u32(T + at::PEER_COUNT, 4)
            .put_u32(T + at::PLAYER_MASK, 0b1111)
            .put_u8(T + HAS_ROSTER, 1)
            .put_u32(T + ROSTER_PLAYERS, 4)
            .put_u8(T + ARBITRATION_ROUND, 1)
            .put_u32(MATCHMAKING_RESULT, 3)
            .put_u32(ARBITRATION_STATE, 2);
        for i in 0..4u32 {
            let p = T + at::PEERS + i * at::PEER_SIZE;
            g.put_u32(p + at::PEER_MAP_STATUS, 4)
                .put_u32(p + at::PEER_HOPPERS + 10 * 4, 1)
                .put_u32(p + PEER_ARBITRATION, 2);
            let id = [i as u8 + 1; ID_SIZE];
            g.put(T + at::PLAYERS + i * at::PLAYER_SIZE, &id)
                .put(T + ROSTER_IDS + i * ID_SIZE as u32, &id);
        }
        g
    }

    fn wait(g: &FakeGuest) -> MatchWait {
        read(g).unwrap().unwrap().expect("a wait")
    }

    #[test]
    fn a_ready_lobby_says_so_and_a_countdown_or_a_game_is_no_wait() {
        let g = full_doubles();
        let w = wait(&g);
        assert_eq!(w.roster, Some((4, 4)));
        assert!(
            w.reason().starts_with("every traced check reads ready"),
            "{}",
            w.line()
        );
        assert_eq!(
            w.facts(),
            "players 4/4, roster 4/4 present, peers 4 (bots 0), maps [4,4,4,4], \
             hopper statuses [1,1,1,1], arbitration [2,2,2,2], round set, \
             matchmaking result 0x3, arbitration state 0x2"
        );
        let mut g = full_doubles();
        g.put_u8(T + COUNTDOWN, 1);
        assert_eq!(read(&g).unwrap(), Ok(None), "counting down");
        for ty in [8, 9] {
            let mut g = full_doubles();
            g.put_u32(SESSION_TYPE, ty);
            assert_eq!(read(&g).unwrap(), Ok(None), "life cycle {ty}");
        }
    }

    #[test]
    fn each_check_is_named_in_the_host_updates_order() {
        let mut g = full_doubles();
        g.put_u32(T + at::PLAYER_COUNT, 3);
        assert!(
            wait(&g)
                .reason()
                .starts_with("the match session has 3 of 4 players")
        );

        let mut g = full_doubles();
        g.put(T + ROSTER_IDS + 3 * ID_SIZE as u32, &[9; ID_SIZE]);
        assert!(
            wait(&g)
                .reason()
                .starts_with("the matchmaking roster lists 4 players and 3 of them")
        );

        let mut g = full_doubles();
        g.put_u32(T + at::PEERS + 2 * at::PEER_SIZE + at::PEER_HOPPERS + 40, 0);
        assert!(
            wait(&g)
                .reason()
                .starts_with("peer 2's status for the hopper is 0"),
            "{}",
            wait(&g).reason()
        );

        let mut g = full_doubles();
        g.put_u32(T + at::PEERS + at::PEER_SIZE + at::PEER_MAP_STATUS, 2);
        assert!(
            wait(&g)
                .reason()
                .starts_with("peer 1 has not loaded the map (map status 2")
        );

        let mut g = full_doubles();
        g.put_u8(T + ARBITRATION_ROUND, 0);
        assert!(wait(&g).reason().contains("no arbitration round"));

        let mut g = full_doubles();
        g.put_u32(T + at::PEERS + PEER_ARBITRATION, 5);
        assert!(
            wait(&g)
                .reason()
                .starts_with("peer 0's arbitration registration failed (status 5")
        );

        let mut g = full_doubles();
        g.put_u32(T + at::PEERS + PEER_ARBITRATION, 1);
        assert!(
            wait(&g)
                .reason()
                .starts_with("arbitration: still registering")
        );
    }

    #[test]
    fn a_lobby_this_console_does_not_host_is_named() {
        let mut g = full_doubles();
        g.put_u32(T + HOST_PEER, 1);
        let r = read(&g).unwrap().unwrap_err();
        assert!(
            r.starts_with("this console hosts no matchmaking lobby (life cycle 6"),
            "{r}"
        );
        let mut g = full_doubles();
        g.put_u32(SESSION_TYPE, 1);
        assert!(read(&g).unwrap().is_err(), "the party's lobby");
    }
}
