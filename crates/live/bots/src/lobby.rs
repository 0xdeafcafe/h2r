//! Why a lobby will not start (docs/game/bots.md §4.2, "The start
//! check"): what the game's start state (`LOBBY_START_STATE`, `0x19a2ce`)
//! reads, read the same way, so the log can say which check holds a
//! Start press back.
//!
//! Start Game (`0x251050`) acts only on start state 9 and says nothing
//! otherwise; the lobby's status line (`0x24ffc9`) shows the state's text.
//! The state's checks, in order:
//!
//! - 0: no squad session in a lobby state (`0x59670`: networking on,
//!   `[0x527364]` with a state; `0x58d70`: the state 3 to 8), or no lobby
//!   (`0x19a279` 0).
//! - 1: the session is not a pregame one (`0x19a279`, from the session
//!   type `[0x527334]`, is not 3).
//! - matchmaking (lobby mode 6, `0x199971`): the hopper's own states
//!   (`0x19a5fd`), 11 and up.
//! - 4, 5, 6: the map (`0x652a0`, each peer's map status at peer `+0x88`):
//!   4 someone is still loading it, 5 a peer failed to, 6 this console
//!   failed to (`[0x4cd8f8]` 1, `0x65280`).
//! - 7: the co-op check (lobby modes 0, 2, 4: exactly two players, both
//!   local) or, in a team game, the teams check (from 1 to the map's most
//!   teams for the variant, `0x251139`).
//! - 10: the countdown runs (`0x64a60`: session `+0x49a8`, `+0x49ac`).
//! - 8: this console is not the party leader (`0x592f0`: session `+0x72d8`
//!   against `+0x50`), "Waiting for ... to start the game".
//! - 9: ready.
//!
use guest_layout::memory::GuestMemory;

use crate::guest::R;

/// The networking byte (`0x59670` and the rest test it first).
pub const NETWORKING: u32 = 0x0052_7330;
/// The squad session type (`0x592d0`): 1 and 2 are a pregame lobby
/// (`0x19a279` makes them 3).
pub const SESSION_TYPE: u32 = 0x0052_7334;
/// A byte `0x19a279` tests before the session type: 0 is no lobby.
pub const LOBBY_ON: u32 = 0x004d_8ba0;
/// 1 when this console failed to load the map (`0x65280`).
pub const MAP_FAILED_HERE: u32 = 0x004c_d8f8;
/// The squad session's address: the party's session, the one a custom
/// game's lobby and game are played in.
pub const SESSION: u32 = 0x0052_7364;
/// The target session's address: the session a matchmaking search hosts
/// or joins, and its game is played in, beside the party's squad session
/// (docs/game/bots.md §8.4, "The matchmaking host's lobby"). `0x65340`
/// picks the squad session for life-cycle types 1 to 4 and this one for
/// 6 to 9 (its table at `0x65520`); `0x59780` prefers it whenever it has
/// a state.
pub const TARGET_SESSION: u32 = 0x0052_736c;
/// Session `+0x741c`: its state; 3 to 8 is a lobby (or a game) the start
/// checks read.
pub const STATE: u32 = 0x741c;
/// Session `+0x50` and `+0x72d8`: the peers `0x592f0` compares; equal on
/// the party leader's console.
pub const LEADER_PEER: u32 = 0x50;
pub const PEER: u32 = 0x72d8;
/// Session `+0x40`: the host's peer; equal to `+0x72d8` on the host.
pub const HOST_PEER: u32 = 0x40;
/// Session `+0x54`: the peer count; the peers at `+0x58`, `0x10c` each,
/// their map status at peer `+0x88` and its progress at `+0x8c`.
pub const PEER_COUNT: u32 = 0x54;
pub const PEERS: u32 = 0x58;
pub const PEER_SIZE: u32 = 0x10c;
pub const PEER_MAP: u32 = 0x88;
/// Session `+0x49a8`: a countdown is set; `+0x49ac` its seconds.
pub const COUNTDOWN: u32 = 0x49a8;

/// The session the game plays in for life-cycle type `session_type`
/// (`0x65340`'s table at `0x65520`): the target session for the
/// matchmaking types 6 to 9 (6 and 7 the matchmaking lobby, 8 its game,
/// 9 after it), else the squad session (1 and 2 a custom game's lobby, 3
/// its game, 4 after it; 0 and 5, which `0x65340` gives none, keep the
/// squad session as before).
pub fn game_session_at(session_type: u32) -> u32 {
    if (6..=9).contains(&session_type) {
        TARGET_SESSION
    } else {
        SESSION
    }
}

/// Whether `session_type` is a matchmaking lobby's (6 or 7: `0x19a279`
/// gives both 2).
pub fn matchmaking_type(session_type: u32) -> bool {
    matches!(session_type, 6 | 7)
}

/// Whether the life cycle is matchmaking's (types 6 to 9: the match's
/// lobby, its game and after it), networking on. A Clankermatch's bots
/// live only there.
pub fn matchmaking_cycle(m: &dyn GuestMemory) -> bool {
    m.u8(NETWORKING).is_ok_and(|n| n != 0)
        && m.u32(SESSION_TYPE).is_ok_and(|t| (6..=9).contains(&t))
}

/// The session the game plays in now ([`game_session_at`] of the
/// life-cycle type `[0x527334]`, read as 0 with networking off), or 0.
pub fn game_session(m: &dyn GuestMemory) -> R<u32> {
    let ty = if m.u8(NETWORKING)? != 0 {
        m.u32(SESSION_TYPE)?
    } else {
        0
    };
    m.u32(game_session_at(ty))
}

/// The start states (`0x19a2ce`), as the status line names them
/// (`0x19a50f`, the strings in `mainmenu.map`).
pub const READY: u32 = 9;
pub const COUNTING_DOWN: u32 = 10;

/// What the start checks read from the session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub lobby_on: bool,
    pub networking: bool,
    pub session_type: u32,
    /// This console failed to load the map.
    pub map_failed_here: bool,
    /// The session's state, when there is one.
    pub state: Option<u32>,
    /// `0x592f0`: this console leads the party.
    pub leader: bool,
    /// The countdown's seconds, when one is set.
    pub countdown: Option<i32>,
    /// Each peer's map status (`1` failed, `2` loading, `3` and `4` done).
    pub maps: Vec<u32>,
    /// In a matchmaking lobby (types 6 and 7): the target session's state,
    /// when there is one ([`TARGET_SESSION`]).
    pub target_state: Option<u32>,
    /// In a matchmaking lobby: this console hosts the target session (its
    /// host peer `+0x40` is its own `+0x72d8`).
    pub target_host: bool,
}

/// Whether `state` is one the start checks take as a lobby (`0x58d70`).
pub fn lobby_state(state: u32) -> bool {
    (3..=8).contains(&state)
}

/// Reads what the start checks read.
pub fn facts(m: &dyn GuestMemory) -> R<Facts> {
    read(m, true)
}

/// The inputs to `joinable`, `pregame_line` and `lobby_players`. Only the
/// start-state diagnostic needs every peer's map status and the countdown.
pub fn membership(m: &dyn GuestMemory) -> R<Facts> {
    read(m, false)
}

fn read(m: &dyn GuestMemory, start_checks: bool) -> R<Facts> {
    let mut f = Facts {
        lobby_on: m.u8(LOBBY_ON)? != 0,
        networking: m.u8(NETWORKING)? != 0,
        session_type: m.u32(SESSION_TYPE)?,
        map_failed_here: start_checks && m.u32(MAP_FAILED_HERE)? == 1,
        ..Facts::default()
    };
    if !f.networking {
        return Ok(f);
    }
    if matchmaking_type(f.session_type) {
        let t = m.u32(TARGET_SESSION)?;
        if t != 0 {
            f.target_state = Some(m.u32(t + STATE)?);
            f.target_host = m.u32(t + HOST_PEER)? == m.u32(t + PEER)?;
        }
    }
    let s = m.u32(SESSION)?;
    if s == 0 {
        return Ok(f);
    }
    let state = m.u32(s + STATE)?;
    f.state = Some(state);
    if !start_checks || !lobby_state(state) {
        return Ok(f);
    }
    f.leader = m.u32(s + PEER)? == m.u32(s + LEADER_PEER)?;
    if m.u8(s + COUNTDOWN)? != 0 {
        f.countdown = Some(m.i32(s + COUNTDOWN + 4)?);
    }
    let peers = m.i32(s + PEER_COUNT)?.clamp(0, 16) as u32;
    for i in 0..peers {
        f.maps.push(m.u32(s + PEERS + i * PEER_SIZE + PEER_MAP)?);
    }
    Ok(f)
}

/// Whether these facts are a pregame lobby's: a squad session of a
/// pregame type (1 or 2: `0x19a279` makes both 3, the type the lobby's
/// input handler `0x250b1e` tests at `0x250cab` before it opens a joining
/// controller's profile chooser) in a lobby state (3 to 8, `0x58d70`).
/// A Live or System Link custom game lobby has one from the moment it is
/// hosted or joined (Split Screen's players join on the players screen
/// whatever it has). A running game keeps the session in these states
/// too, so a caller asks about the game itself as well.
pub fn pregame(f: &Facts) -> bool {
    f.lobby_on
        && f.networking
        && matches!(f.session_type, 1 | 2)
        && f.state.is_some_and(lobby_state)
}

/// Whether these facts are a matchmaking lobby this console hosts: the
/// life cycle in a matchmaking lobby type (6 or 7, [`matchmaking_type`])
/// with the target session ([`TARGET_SESSION`], where the search hosted)
/// in a lobby state and this console its host. That is a Clankermatch's
/// lobby: the empty search hosts it (§8.4), and the hopper's countdown
/// starts it once the target session has its players.
///
/// Retail adds a local player there as anywhere: the per-frame
/// local-player pass (`0x63d80` → `0x65770`) runs on every session with a
/// state, the target session too, and adds each signed-in local user it
/// has no player for, whatever the life-cycle type (`0x646b0` and
/// `0x65a60` read the session's state and room, never the type). The
/// matchmaking screen (`0xd2`, constructor `0x2519f7`, input `0x251afa`)
/// takes only B and Back (and X), so a second controller cannot sign in
/// there, and the custom lobby's join press (`0x250c80`) refuses outside
/// type 1 (`0x19a902`, `0x19a279`). A bot signs in with the game's own
/// sign-in, presses nothing, and the pass adds it.
pub fn matchmaking_host(f: &Facts) -> bool {
    f.lobby_on
        && f.networking
        && matchmaking_type(f.session_type)
        && f.target_state.is_some_and(lobby_state)
        && f.target_host
}

/// Whether a bot may join the lobby these facts read: a pregame lobby
/// ([`pregame`]) or a matchmaking lobby this console hosts
/// ([`matchmaking_host`]).
pub fn joinable(f: &Facts) -> bool {
    pregame(f) || matchmaking_host(f)
}

/// The log line for what [`joinable`] read, so a run shows which fact
/// held a lobby back.
pub fn pregame_line(f: &Facts) -> String {
    let target = if matchmaking_type(f.session_type) {
        format!(
            ", match session state {}{}",
            f.target_state
                .map_or_else(|| "none".to_owned(), |s| s.to_string()),
            if f.target_host {
                " (this console hosts it)"
            } else {
                " (another console hosts it)"
            }
        )
    } else {
        String::new()
    };
    format!(
        "bots: lobby facts: lobby {}, networking {}, session type {}, session state {}{target}: {}",
        f.lobby_on,
        f.networking,
        f.session_type,
        f.state.map_or_else(|| "none".to_owned(), |s| s.to_string()),
        if pregame(f) {
            "a pregame lobby"
        } else if matchmaking_host(f) {
            "a matchmaking lobby this console hosts"
        } else {
            "no pregame lobby"
        }
    )
}

/// [`pregame`] over guest memory; a read that faults is no lobby.
pub fn in_pregame(m: &dyn GuestMemory) -> bool {
    membership(m).is_ok_and(|f| pregame(&f))
}

/// [`joinable`] over guest memory; a read that faults is no lobby.
pub fn in_joinable(m: &dyn GuestMemory) -> bool {
    membership(m).is_ok_and(|f| joinable(&f))
}

/// Session `+0x1118`: the session's player count, what the matchmaking
/// start counts (`0x66050`, the match check `0x7e2df`).
pub const PLAYER_COUNT: u32 = 0x1118;

/// The players in the lobby a bot may join ([`joinable`]), as the game
/// counts them: the game session's player count ([`game_session`],
/// `+0x1118`). `None` out of such a lobby.
pub fn lobby_players(m: &dyn GuestMemory, f: &Facts) -> R<Option<u32>> {
    if !joinable(f) {
        return Ok(None);
    }
    let s = game_session(m)?;
    if s == 0 {
        return Ok(None);
    }
    Ok(Some(m.u32(s + PLAYER_COUNT)?))
}

/// The people in the party of a matchmaking lobby this console hosts
/// ([`matchmaking_host`]): the squad session's player count (`+0x1118`,
/// what `0x66050` holds against the hopper's party bounds). Every console
/// of the party and each of its guests has a player there, and no phantom
/// does: a Clankermatch's phantoms join the target session only
/// (docs/game/bots.md §8.4, "Phantoms only"). `None` out of such a lobby,
/// or with the squad session out of its lobby states.
pub fn party_players(m: &dyn GuestMemory, f: &Facts) -> R<Option<u32>> {
    if !matchmaking_host(f) || !f.state.is_some_and(lobby_state) {
        return Ok(None);
    }
    let s = m.u32(SESSION)?;
    if s == 0 {
        return Ok(None);
    }
    let n = m.u32(s + PLAYER_COUNT)?;
    Ok((1..=16).contains(&n).then_some(n))
}

/// The log line for [`lobby_players`]: what `h2 bots test` reads as the
/// lobby's own count (the Live server's count of a session is what the
/// host told it, not the game's).
pub fn players_line(n: u32) -> String {
    format!("bots: lobby players: {n} in the game's session")
}

/// The map check (`0x652a0`) over the peers of a session in a lobby
/// state: 1 when a peer failed to load the map, 2 when one is still
/// loading it, 3 when one has it and none is loading, else 4.
pub fn map_status(maps: &[u32]) -> u32 {
    let mut out = 4;
    for &s in maps {
        match s {
            1 => return 1,
            2 => out = 2,
            3 if out == 4 => out = 3,
            _ => {}
        }
    }
    out
}

/// The start state the session-level checks give (`0x19a2ce` without the
/// matchmaking, co-op and teams checks, which read the lobby mode, the
/// variant and the map): what the state is unless one of those says 7 or
/// a hopper state.
pub fn expected(f: &Facts) -> u32 {
    // 0x19a279: no lobby, or a session type that is none.
    if !f.lobby_on || !f.networking || f.session_type == 0 {
        return 0;
    }
    // 0x59670, 0x58d70: a session in a lobby state.
    match f.state {
        Some(s) if s != 0 && lobby_state(s) => {}
        _ => return 0,
    }
    if !matches!(f.session_type, 1 | 2) {
        return 1;
    }
    let mut state = READY;
    match map_status(&f.maps) {
        1 => return if f.map_failed_here { 6 } else { 5 },
        2 => state = 4,
        _ => {}
    }
    // The tail (0x19a4c5): the countdown, then the leader.
    if f.countdown.is_some_and(|c| c >= 0) {
        COUNTING_DOWN
    } else if f.leader {
        READY
    } else if state == READY {
        8
    } else {
        state
    }
}

/// What holds a start back at `state` (the game's own number), with what
/// [`facts`] read where it says more.
pub fn blocked_by(state: u32, f: &Facts) -> String {
    match state {
        READY => "nothing (ready)".to_owned(),
        COUNTING_DOWN => format!(
            "nothing (the countdown runs: {})",
            f.countdown.map_or_else(|| "?".to_owned(), |c| c.to_string())
        ),
        0 => format!(
            "no squad session in a lobby (lobby {}, networking {}, session state {})",
            f.lobby_on,
            f.networking,
            f.state
                .map_or_else(|| "none".to_owned(), |s| s.to_string())
        ),
        1 => format!(
            "the session is not a pregame lobby (type {})",
            f.session_type
        ),
        4 => format!(
            "the map: a peer is still loading it (map status {:?})",
            f.maps
        ),
        5 => format!("the map: a peer failed to load it (map status {:?})", f.maps),
        6 => "the map: this console failed to load it".to_owned(),
        7 => "the teams or co-op check (too many teams for the map, or co-op without exactly two players on this console)".to_owned(),
        8 => "the party leader check: this console does not lead the party (session +0x72d8 is not +0x50)".to_owned(),
        n => format!("the matchmaking check (state {n})"),
    }
}

pub mod match_wait;

#[cfg(test)]
mod tests;
