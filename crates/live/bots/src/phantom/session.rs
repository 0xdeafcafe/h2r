//! A phantom's machine and player, written into the host's session the way
//! the game's own join path writes them (docs/game/bots.md §5.1), and its
//! actions written into its simulation slot. Pure memory work on
//! [`GuestMemory`], so the tests run on synthetic sessions.
//!
//! The game does the rest from the session: each frame it rebuilds the
//! machine set from the peers (`0x83f00`), diffs the session players
//! (`0x83610`) and creates the player and its simulation slot (`0x85140`).
//!
//! The same writes put a phantom in the pregame lobby this console hosts
//! ([`host_lobby`]), with the peer properties a real peer sends
//! ([`sync_props`]: this console's own map and hopper statuses, so the
//! start checks pass) and even teams ([`lobby_team`], [`rebalance`]).

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;

use crate::guest;

pub(super) mod lobby_membership;
pub(super) mod match_roster;

pub type R<T> = Result<T, MemFault>;

/// Session offsets (`*SQUAD_SESSION`).
pub mod at {
    pub const UPDATE: u32 = 0x4c;
    pub const PEER_COUNT: u32 = 0x54;
    pub const PEERS: u32 = 0x58;
    pub const PEER_SIZE: u32 = 0x10c;
    pub const PLAYER_COUNT: u32 = 0x1118;
    pub const PLAYER_MASK: u32 = 0x111c;
    pub const PLAYERS: u32 = 0x1120;
    pub const PLAYER_SIZE: u32 = 0x13c;
    /// Must be 4 for the player diff (`0x83610`).
    pub const CLASS: u32 = 0x4980;
    pub const MAX_PEERS: u32 = 0x4990;
    pub const MAX_PLAYERS: u32 = 0x4994;
    /// The game variant the lobby plays (`0x130` bytes, a
    /// `variants::record`); its flags at `+0x48`, bit 0 team play (the
    /// host's configuration pass `0x65cb0` tests it at `0x65ec8`).
    pub const VARIANT: u32 = 0x4db0;
    pub const VARIANT_FLAGS: u32 = 0x48;
    /// Join reservations, 16 × `0x24`: `+0` held, `+1` taken. A held one
    /// not yet taken keeps a player slot free (`0x62e30`).
    pub const RESERVATIONS: u32 = 0x7668;
    pub const RESERVATION_SIZE: u32 = 0x24;
    pub const CHANNELS: u32 = 0x72dc;
    pub const CHANNEL_SIZE: u32 = 0x14;
    pub const STATE: u32 = 0x741c;
    pub const CHANGES: u32 = 0x7618;
    /// The session teardown at 0x5a575 advances this even when the
    /// next lobby reuses the same session address.
    pub const INCARNATION: u32 = 0x7650;
    /// The host's peer index. A joining client sets it to 0 and its own
    /// peer to 1 (`0x61050`: the host's address is peer 0, its own
    /// peer 1); becoming the host (state 5, `0x61390` at `0x6141a`) sets
    /// it to [`LOCAL_PEER`]. The removal (`0x5fe20`) moves it down.
    pub const HOST_PEER: u32 = 0x40;
    /// The party leader's peer index (the lobby's leader check `0x592f0`
    /// compares it with [`LOCAL_PEER`]).
    pub const PEER_50: u32 = 0x50;
    /// This console's own peer index (skipped for views).
    pub const LOCAL_PEER: u32 = 0x72d8;
    /// The protocol `0x6ec80` reads: 0 local, 1 synchronous, 2 distributed.
    pub const PROTOCOL: u32 = 0x4dac;

    /// In a peer.
    pub const PEER_MAC: u32 = 0xa;
    /// A byte: the peer's properties came (the host's properties message
    /// `0x5ed90` sets it for a remote peer, `0x5a790` for its own).
    pub const PEER_PROPS_SET: u32 = 0x24;
    /// The peer's `0xc8`-byte properties, which each console makes of its
    /// own state (`0x658c0`) and sends to the host (message `0x1a`): the
    /// host copies them in whole (`0x5ed90`, `rep movsd` of `0x32`
    /// dwords).
    pub const PEER_PROPS: u32 = 0x28;
    pub const PROPS_SIZE: u32 = 0xc8;
    /// Properties `+0`: the name (15 units and a NUL, from `0x4cd86a`).
    pub const PEER_NAME: u32 = 0x28;
    pub const PEER_NAME_UNITS: u32 = 15;
    /// Properties `+0x20`: a second name (31 units and a NUL, from
    /// `0x4cd88a`; both set by `0x645d0` from the lobby's profiles).
    pub const PEER_NAME2: u32 = 0x48;
    pub const PEER_NAME2_UNITS: u32 = 31;
    /// Properties `+0x60`: the map status (`[0x4cd8f8]`: 1 failed, 2
    /// loading, 3 and 4 loaded), `+0x64` its progress (`[0x4cd8fc]`).
    pub const PEER_MAP_STATUS: u32 = 0x88;
    /// Properties `+0x84`: each hopper's status, 16 dwords
    /// (`0x192df0` per hopper); the matchmaking start (`0x66050`) wants 1
    /// for the lobby's hopper on every peer.
    pub const PEER_HOPPERS: u32 = 0xac;
    pub const PEER_PLAYERS: u32 = 0xf8;
    pub const PEER_SLOTS: u32 = 0xfc;

    /// In a session player.
    pub const P_ID: u32 = 0;
    pub const P_PEER: u32 = 0xc;
    pub const P_USER: u32 = 0x10;
    pub const P_CONTROLLER: u32 = 0x14;
    pub const P_REQUESTED: u32 = 0x18;
    pub const P_EFFECTIVE: u32 = 0xa8;
    pub const P_138: u32 = 0x138;

    /// The simulation world (`*0x4cf77c`): its mode ([`super::world_mode`],
    /// not the game's), and the player slots.
    pub const SIM_GLOBALS: u32 = 0x004c_f77c;
    pub const SIM_MODE: u32 = 8;
    /// The world's state (`+0x18`): on a server 3 while the game starts
    /// and waits for every machine's view (`0x69c80`), 4 while views
    /// join (`0x69d50`), 5 running (`0x69dd0`).
    pub const SIM_STATE: u32 = 0x18;
    /// The machines the start waits for (`+0x20`; `0x6b1f0` sets it to the
    /// whole machine set, and `+0x1c` the time it began).
    pub const SIM_WAIT: u32 = 0x20;
    /// Pointer to the machine set (the world's `+0`, `[0x4cf780]`): its
    /// valid mask at `+0x1c`, its machine ids at `+0x24`, 6 bytes each
    /// (`0x83b80` finds one), rebuilt from the session's peers
    /// (`0x83f00`), so machine `i` is peer `i`.
    pub const MACHINES: u32 = 0x004c_f780;
    pub const MACHINES_MASK: u32 = 0x1c;
    pub const MACHINES_IDS: u32 = 0x24;
    pub const SIM_SLOTS: u32 = 0x7c;
    pub const SLOT_SIZE: u32 = 0x88;
    pub const SLOT_MAC: u32 = 0x18;
    pub const SLOT_STAMP: u32 = 0x28;
    pub const SLOT_ACTION: u32 = 0x2c;
    pub const ACTION_SIZE: usize = 0x5c;

    /// Player datum `+0xd4`: a copy of its configuration.
    pub const PLAYER_CONFIG: u32 = 0xd4;
    pub const CONFIG_SIZE: usize = 0x90;
    pub const CONFIG_TEAM: usize = 0x7c;
    // 0x65e37 passes 32 units to the name builder. The look starts at +0x40.
    pub const CONFIG_NAME_UNITS: usize = guest_layout::view::players::NAME_UNITS;
    /// The retail profile appearance inside a player configuration (0x18fd94).
    pub const CONFIG_LOOK: usize = 0x40;
    /// The configuration's handicap: 0 none, 1 to 3 the shields scaled to
    /// 0.75, 0.5 and 0.25 (`0x1588b0`'s table at `0x15897c`). The local
    /// builder `0x18fd94` takes it from slot `+0x200`, which the handicap
    /// screen sets (`0x2b4c45`); docs/game/bots.md §5.1, "A phantom's
    /// player configuration".
    pub const CONFIG_HANDICAP: usize = 0x7d;
    pub const HANDICAP_NONE: u8 = 0;
    /// The level byte (`0x18fd94` takes it from slot `+0x204`, `0x14bc80`
    /// clamps it to 0..7); the matchmaking start (`0x66050`) wants each
    /// player's at least the hopper's.
    pub const CONFIG_LEVEL: usize = 0x81;
    /// Unit `+0x212`: the current weapon index.
    pub const UNIT_WEAPON: u32 = 0x212;
}

use at::*;

/// The lobby state, which a running game keeps too (states 6 to 8 tear the
/// simulation down; docs/game/bots.md §5.1).
pub const STATE_LIVE: u32 = 5;
/// The game's mode for the distributed server: what `0x6ec80` puts in the
/// game options (`+4`) and the game globals (`[0x4e6948]+0xc`): 1 local,
/// 2 synchronous client, 3 synchronous server, 4 distributed client, 5
/// distributed server.
pub const GAME_DIST_SERVER: u32 = 5;
/// The simulation world's mode for the distributed server (`[0x4cf77c]+8`,
/// what [`host_session`] reads). The world numbers its modes otherwise
/// than the game: see [`world_mode`].
pub const WORLD_DIST_SERVER: u32 = 4;
/// The local mode, 1 in both numberings (Split Screen and campaign alone).
pub const MODE_LOCAL: u32 = 1;

/// The simulation world's mode for a game mode, as the world's setup
/// (`0x68c00`) maps it through its jump table at `0x68da8`: 1 → 1, 2 → 3,
/// 3 → 2, 4 → 5, 5 → 4. So the world's 2 and 4 are the servers and 3 and
/// 5 the clients. The game's other writers agree: becoming the host
/// (`0x6aee0`) sets the world's mode 4 and the game's 5 together; becoming
/// a client (`0x6ae20`) sets the game's 2 or 4 and builds the world anew
/// through `0x68c00`; the action store (`0x847d0`) takes the local and
/// synchronous server worlds (1, 2) as authoritative. `None` for none.
pub fn world_mode(game: u32) -> Option<u32> {
    match game {
        1 => Some(1),
        2 => Some(3),
        3 => Some(2),
        4 => Some(5),
        5 => Some(4),
        _ => None,
    }
}

/// A simulation world mode's name.
pub fn world_mode_name(world: u32) -> &'static str {
    match world {
        0 => "none",
        1 => "local",
        2 => "synchronous server",
        3 => "synchronous client",
        4 => "distributed server",
        5 => "distributed client",
        _ => "unknown",
    }
}
pub const MAX: u32 = 16;

/// Why the host cannot take a phantom now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotNow {
    /// No game running and no lobby to join: wait.
    Wait,
    /// A game runs, but this console is not (or not yet) its host, and
    /// what the check read. A phantom waits and tries again: in the first
    /// frames of a game, and in a game this console joined, this is so.
    NotHost(String),
    /// No room for another peer or player.
    Full,
}

/// The host's session, when a phantom can join now: a multiplayer game
/// running, this console its distributed server, the session live.
pub fn host_session(m: &dyn GuestMemory) -> Result<u32, NotNow> {
    let running = guest::game_tick(m).is_some() && multiplayer(m);
    if !running {
        return Err(NotNow::Wait);
    }
    let read = |a: u32| m.u32(a).ok();
    let ready = m.u8(points::NETWORK_READY.base).ok().unwrap_or(0) != 0;
    // The game's session: the squad's in a custom game, the target
    // session in a matchmade one (`crate::lobby::game_session`).
    let s = crate::lobby::game_session(m).unwrap_or(0);
    let sim = read(SIM_GLOBALS).unwrap_or(0);
    if !ready || s == 0 || sim == 0 {
        // A game with no network session: a local game, no host.
        return Err(NotNow::NotHost(format!(
            "no network session (network ready {ready}, session {s:#x}, simulation {sim:#x})"
        )));
    }
    let shown = |v: Option<u32>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
    // The world's mode, not the game's (`world_mode`): the run
    // 20261119-014620 read 4, the distributed server, and took it for a
    // client.
    let mode = read(sim + SIM_MODE);
    if mode == Some(MODE_LOCAL) {
        // Split Screen: its session is of the local type (session `+0x18`
        // 0, made by `0x59890(0, …)` from the players screen in mode 1),
        // which `0x64060` gives protocol 0, and `0x6ec80` game mode 1
        // (world mode 1). No game of it has peers; a hosted System Link or Live game is
        // distributed (protocol 2 for every shipped engine), one console
        // or many.
        return Err(NotNow::NotHost(
            "a local game (simulation mode 1, Split Screen): phantoms join System Link and Xbox Live games only"
                .into(),
        ));
    }
    if mode != Some(WORLD_DIST_SERVER) {
        // 0x6ec80 chose from local peer == host peer: say both.
        return Err(NotNow::NotHost(format!(
            "the simulation is not the distributed server (world mode {}, {}; the server's is \
             {WORLD_DIST_SERVER}; session host peer {}, this console's peer {}, peer count {})",
            shown(mode),
            mode.map_or("unreadable", world_mode_name),
            shown_index(read(s + HOST_PEER)),
            shown_index(read(s + LOCAL_PEER)),
            shown(read(s + PEER_COUNT)),
        )));
    }
    let state = read(s + STATE);
    if state != Some(STATE_LIVE) {
        return Err(NotNow::NotHost(format!(
            "the session is not live (state {}, live is {STATE_LIVE})",
            shown(state)
        )));
    }
    let class = read(s + CLASS);
    if class != Some(4) {
        return Err(NotNow::NotHost(format!(
            "the session's class is {}, not 4",
            shown(class)
        )));
    }
    Ok(s)
}

/// A peer index as the log shows it: -1 for none.
fn shown_index(v: Option<u32>) -> String {
    v.map_or_else(|| "?".to_owned(), |v| (v as i32).to_string())
}

/// The session states in which this console is the session's host
/// (`0x58d20`; state 5 is entered by `0x61390`, which makes the local
/// peer the host).
pub const HOST_STATES: std::ops::RangeInclusive<u32> = 5..=8;
/// The distributed protocol (`+0x4dac`), every shipped multiplayer engine's.
pub const PROTOCOL_DISTRIBUTED: u32 = 2;

/// What the session game start (`0x6ec80`) reads to choose the game mode
/// (the simulation world's follows from it, [`world_mode`]), as the bots'
/// hook before it reads them: the game mode is 1 for
/// protocol 0, `2 + is_host` for 1 and `4 + is_host` for 2, with `is_host`
/// the local peer (`+0x72d8`) equal to the host peer (`+0x40`,
/// `0x6ee60..0x6ee71`, `0x6eeba..0x6eecd`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartFacts {
    pub state: u32,
    pub protocol: u32,
    pub peers: u32,
    pub host: u32,
    pub local: u32,
    /// Peers other than this console's that are not phantoms (their
    /// machine id is not ours, [`crate::phantom::handle_of`]).
    pub consoles: u32,
}

impl StartFacts {
    pub fn read(m: &dyn GuestMemory, s: u32) -> R<StartFacts> {
        let peers = m.u32(s + PEER_COUNT)?;
        let local = m.u32(s + LOCAL_PEER)?;
        let mut consoles = 0;
        for p in 0..peers.min(MAX) {
            if p != local && crate::phantom::handle_of(mac_at(m, peer(s, p) + PEER_MAC)?).is_none()
            {
                consoles += 1;
            }
        }
        Ok(StartFacts {
            state: m.u32(s + STATE)?,
            protocol: m.u32(s + PROTOCOL)?,
            peers,
            host: m.u32(s + HOST_PEER)?,
            local,
            consoles,
        })
    }

    /// The game mode `0x6ec80` will choose from these, when it starts the
    /// game (not the world's: [`world_mode`]).
    pub fn mode(&self) -> Option<u32> {
        let host = u32::from(self.host == self.local);
        match self.protocol {
            0 => Some(MODE_LOCAL),
            1 => Some(2 + host),
            2 => Some(4 + host),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "session state {}, protocol {}, peer count {} ({} other consoles), host peer {}, this console's peer {}",
            self.state,
            self.protocol as i32,
            self.peers,
            self.consoles,
            self.host as i32,
            self.local as i32,
        )
    }
}

/// What the bots' hook does before a game starts from the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostFix {
    /// Leave the session as it is, and why.
    Keep(String),
    /// Phantoms are planned and the session is this console's alone, but
    /// its host peer is not its own: make it so (`+0x40` = `local`), as
    /// becoming the host does (`0x61390`), so the game starts as the
    /// distributed server (game mode 5, world mode 4) and phantoms can
    /// join.
    Host { host: u32, local: u32 },
}

/// Decides [`HostFix`] for a game start from `f` with phantoms `planned`.
/// Only a distributed session in a host state (5 to 8) with no other
/// console in it is changed: there the host can only be this console, so
/// a host peer that is not its own is wrong, and the game would start as
/// a distributed client of nobody (game mode 4). Runs 20261119-005001 and
/// -014620 never needed it: their world mode 4 was the distributed server
/// all along (see [`world_mode`]); this stays for a session that really
/// has the wrong host.
pub fn host_fix(f: &StartFacts, planned: bool) -> HostFix {
    if !planned {
        return HostFix::Keep("no phantoms planned".into());
    }
    if f.protocol != PROTOCOL_DISTRIBUTED {
        return HostFix::Keep(format!(
            "protocol {}, not distributed: phantoms join System Link and Xbox Live games only",
            f.protocol as i32
        ));
    }
    if f.host == f.local {
        return HostFix::Keep(
            "this console is the session's host: the game starts as the distributed server".into(),
        );
    }
    if !HOST_STATES.contains(&f.state) {
        return HostFix::Keep(format!(
            "session state {} is not a host's (5 to 8): this console joined someone's game",
            f.state
        ));
    }
    if f.local >= f.peers.min(MAX) {
        return HostFix::Keep(format!(
            "this console's peer {} is not one of the {} peers",
            f.local as i32, f.peers
        ));
    }
    if f.consoles > 0 {
        return HostFix::Keep(format!(
            "{} other console{} in the session: its host stays",
            f.consoles,
            if f.consoles == 1 { "" } else { "s" }
        ));
    }
    HostFix::Host {
        host: f.host,
        local: f.local,
    }
}

/// Makes this console the session's host (`+0x40` = `local`), as
/// [`HostFix::Host`] says.
pub fn make_host(m: &mut dyn GuestMemory, s: u32, local: u32) -> R<()> {
    m.set_u32(s + HOST_PEER, local)
}

/// The bots' hook before the session game start (`SESSION_GAME_START`,
/// `0x6ec80`, EDX the session `s`), with phantoms `planned`: says what the
/// start will read and, when [`host_fix`] says so, makes this console the
/// session's host first. Lines for the log.
pub fn game_start(m: &mut dyn GuestMemory, s: u32, planned: bool) -> Vec<String> {
    let f = match StartFacts::read(&*m, s) {
        Ok(f) => f,
        Err(e) => {
            return vec![format!(
                "bots: game start: the session at {s:#x} faulted at {:#x}",
                e.addr
            )];
        }
    };
    let mode = f.mode().map_or_else(
        || "none".to_owned(),
        |g| match world_mode(g) {
            Some(w) => format!("{g} (simulation world mode {w}, {})", world_mode_name(w)),
            None => g.to_string(),
        },
    );
    let mut log = vec![format!(
        "bots: game start: {}: game mode {mode}",
        f.describe()
    )];
    match host_fix(&f, planned) {
        HostFix::Keep(why) => log.push(format!("bots: game start: session as it is ({why})")),
        HostFix::Host { host, local } => match make_host(m, s, local) {
            Ok(()) => log.push(format!(
                "bots: game start: phantoms planned and no other console in the session: \
                 this console hosts it (host peer {} -> {local}), so the game starts as the \
                 distributed server (game mode {GAME_DIST_SERVER}, world mode \
                 {WORLD_DIST_SERVER})",
                host as i32
            )),
            Err(e) => log.push(format!(
                "bots: game start: could not make this console the host (fault at {:#x})",
                e.addr
            )),
        },
    }
    log
}

/// The live network session, game or lobby: the session whose peers a
/// game's end takes phantoms out of. The game's session for the life
/// cycle ([`crate::lobby::game_session`]): the squad session in a custom
/// game, the target session in matchmaking (its lobby and its game).
pub fn live_session(m: &dyn GuestMemory) -> Option<u32> {
    if m.u8(points::NETWORK_READY.base).ok()? == 0 {
        return None;
    }
    let s = crate::lobby::game_session(m).ok().filter(|&s| s != 0)?;
    (m.u32(s + STATE).ok()? == STATE_LIVE).then_some(s)
}

/// The pregame lobby this console hosts, when a phantom can join it now
/// (docs/game/bots.md §5.1, "Phantoms in the pregame lobby"): a pregame
/// lobby's squad session ([`crate::lobby::pregame`]: session type 1 or 2,
/// a lobby state), or a matchmaking lobby's target session this console
/// hosts ([`crate::lobby::matchmaking_host`]: type 6 or 7, a Clankermatch
/// after its empty search, §8.4), that is live (state 5), distributed
/// (protocol 2: a System Link or Xbox Live lobby, not Split Screen's
/// local one) and this console's (the host peer `+0x40` is its own
/// `+0x72d8`), with no game running ([`host_session`] takes that). A real peer joins such a lobby
/// through the host's join (`0x5c720`): a peer (`0x5fbd0`), then its
/// properties (`0x5ed90`) and players (`0x60000`); a phantom gets the
/// same writes.
pub fn host_lobby(m: &dyn GuestMemory) -> Result<u32, NotNow> {
    if guest::game_tick(m).is_some() && multiplayer(m) {
        return Err(NotNow::Wait);
    }
    let Ok(f) = crate::lobby::membership(m) else {
        return Err(NotNow::Wait);
    };
    if !crate::lobby::joinable(&f) {
        return Err(NotNow::Wait);
    }
    let s = crate::lobby::game_session(m).unwrap_or(0);
    let read = |a: u32| m.u32(a).ok();
    let shown = |v: Option<u32>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
    let state = read(s + STATE);
    if state != Some(STATE_LIVE) {
        return Err(NotNow::NotHost(format!(
            "the lobby's session is not live (state {}, live is {STATE_LIVE})",
            shown(state)
        )));
    }
    let protocol = read(s + PROTOCOL);
    if protocol != Some(PROTOCOL_DISTRIBUTED) {
        return Err(NotNow::NotHost(format!(
            "the lobby's session is not distributed (protocol {}): phantoms join System Link \
             and Xbox Live lobbies only",
            shown(protocol)
        )));
    }
    let (host, local) = (read(s + HOST_PEER), read(s + LOCAL_PEER));
    if host.is_none() || host != local {
        return Err(NotNow::NotHost(format!(
            "this console does not host it (session host peer {}, this console's peer {})",
            shown_index(host),
            shown_index(local)
        )));
    }
    Ok(s)
}

/// The live distributed session this console hosts, game or lobby, if
/// any: phantoms already in it stay while it lasts (between a lobby and
/// its game's first hosted frame, and from the game back to the lobby).
pub fn owned_session(m: &dyn GuestMemory) -> Option<u32> {
    let s = live_session(m)?;
    let host = m.u32(s + HOST_PEER).ok()?;
    (host == m.u32(s + LOCAL_PEER).ok()? && m.u32(s + PROTOCOL).ok()? == PROTOCOL_DISTRIBUTED)
        .then_some(s)
}

/// Whether this console hosts a matchmade match's session now, in any
/// state: the life cycle matchmaking's (types 6 to 9, networking on,
/// [`crate::lobby::matchmaking_cycle`]), its target session there with a
/// state, distributed, and this console its host. Between the lobby's
/// advertisement going and the first frame this console serves the game
/// ([`serves_game`]) the session leaves state 5 for arbitration and the
/// handoff (state 7) while the map loads: a Clankermatch's phantoms are
/// still its peers then, and its fill holds (docs/game/bots.md §8.4).
pub fn hosts_match(m: &dyn GuestMemory) -> bool {
    if !crate::lobby::matchmaking_cycle(m) {
        return false;
    }
    let s = match crate::lobby::game_session(m) {
        Ok(s) if s != 0 => s,
        _ => return false,
    };
    let read = |a: u32| m.u32(a).ok();
    let host = read(s + HOST_PEER);
    read(s + STATE).is_some_and(|state| state != 0)
        && read(s + PROTOCOL) == Some(PROTOCOL_DISTRIBUTED)
        && host.is_some()
        && host == read(s + LOCAL_PEER)
}

/// The player slots free for a player with no reservation, as the
/// host's add (`0x60000` → `0x62e30`) counts them: the session's most
/// players (`+0x4994`) less its players (`+0x1118`) less the
/// reservations held and not yet taken (`+0x7668`, 16 × `0x24`).
pub fn free_player_slots(m: &dyn GuestMemory, s: u32) -> R<u32> {
    let mut held = 0;
    for i in 0..MAX {
        let r = s + RESERVATIONS + i * RESERVATION_SIZE;
        if m.u8(r)? != 0 && m.u8(r + 1)? == 0 {
            held += 1;
        }
    }
    let max = m.u32(s + MAX_PLAYERS)?;
    let count = m.u32(s + PLAYER_COUNT)?;
    Ok(max.saturating_sub(count).saturating_sub(held))
}

/// This console's own peer in `s`, if its index is one of the peers.
fn local_peer(m: &dyn GuestMemory, s: u32) -> R<Option<u32>> {
    let local = m.u32(s + LOCAL_PEER)?;
    Ok((local < m.u32(s + PEER_COUNT)?.min(MAX)).then_some(local))
}

/// The two fixed-width peer names and the effective player name/appearance,
/// all derived from the same immutable gamertag when the seat is registered.
pub(super) struct PeerIdentity {
    names: [u8; (PEER_MAP_STATUS - PEER_PROPS) as usize],
    player: [u8; CONFIG_LOOK + 8],
}

impl PeerIdentity {
    pub(super) fn new(name: &str) -> Self {
        let mut names = [0; (PEER_MAP_STATUS - PEER_PROPS) as usize];
        for (start, units) in [(PEER_NAME, PEER_NAME_UNITS), (PEER_NAME2, PEER_NAME2_UNITS)] {
            let field = &mut names[(start - PEER_PROPS) as usize..];
            for (dst, unit) in field.chunks_exact_mut(2).zip(name.encode_utf16().take(units as usize)) {
                dst.copy_from_slice(&unit.to_le_bytes());
            }
        }
        Self { names, player: identity_bytes(name) }
    }
}

type Properties = [u8; PROPS_SIZE as usize];

pub(super) fn host_properties(m: &dyn GuestMemory, s: u32) -> R<Option<Properties>> {
    let Some(local) = local_peer(m, s)? else { return Ok(None) };
    let mut p = [0; PROPS_SIZE as usize];
    m.read(peer(s, local) + PEER_PROPS, &mut p)?;
    Ok(Some(p))
}

fn named_properties(mut host: Properties, identity: &PeerIdentity) -> Properties {
    host[..identity.names.len()].copy_from_slice(&identity.names);
    host
}

/// A phantom's peer properties: this console's own (its map and hopper
/// statuses, which the lobby's start checks read of every peer: the map
/// `0x652a0`, the countdown's end `0x6e410`, the matchmaking start
/// `0x66050`), with `name` for both names. `None` while this console has
/// no peer.
pub fn props_for(m: &dyn GuestMemory, s: u32, name: &str) -> R<Option<Vec<u8>>> {
    Ok(host_properties(m, s)?.map(|host| named_properties(host, &PeerIdentity::new(name)).to_vec()))
}

/// Gives peer `p` (a phantom's, `name`) [`props_for`]'s properties, as the
/// host's properties message (`0x5ed90`) does for a remote peer: the
/// properties set (`+0x24` 1), the `0xc8` bytes in, both counters + 1 when
/// either changed. Kept each frame, so a map load or a hopper's status on
/// this console is the phantom's too. Returns whether it wrote.
pub fn sync_props(m: &mut dyn GuestMemory, s: u32, p: u32, name: &str) -> R<bool> {
    let host = host_properties(m, s)?;
    sync_peer(m, s, p, host.as_ref(), &PeerIdentity::new(name))
}

pub(super) fn sync_peer(
    m: &mut dyn GuestMemory, s: u32, p: u32, host: Option<&Properties>, identity: &PeerIdentity,
) -> R<bool> {
    let wrote = write_peer(m, s, p, host, identity)?.is_some();
    if wrote {
        bump(m, s)?;
    }
    Ok(wrote)
}

/// [`sync_peer`] without the membership bump, for a caller that writes
/// several records in one frame and bumps once ([`bump`]). Returns `None`
/// when nothing was written, else a mask of the 8-byte chunks of the
/// properties that differed (bit `i`: bytes `8i..8i + 8`; 0 when only the
/// properties-set byte was missing).
pub(super) fn write_peer(
    m: &mut dyn GuestMemory, s: u32, p: u32, host: Option<&Properties>, identity: &PeerIdentity,
) -> R<Option<u32>> {
    let Some(host) = host else { return Ok(None) };
    let want = named_properties(*host, identity);
    let a = peer(s, p);
    let mut have = [0; PROPS_SIZE as usize];
    m.read(a + PEER_PROPS, &mut have)?;
    let chunks = props_diff_chunks(&have, &want);
    if chunks == 0 && m.u8(a + PEER_PROPS_SET)? == 1 {
        return Ok(None);
    }
    m.set_u8(a + PEER_PROPS_SET, 1)?;
    m.write(a + PEER_PROPS, &want)?;
    Ok(Some(chunks))
}

/// Which 8-byte chunks of two property blocks differ (bit `i`: bytes
/// `8i..8i + 8`; the `0xc8` bytes are 25 chunks).
pub(super) fn props_diff_chunks(have: &Properties, want: &Properties) -> u32 {
    have.chunks(8)
        .zip(want.chunks(8))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .fold(0, |mask, (i, _)| mask | 1 << i)
}

/// This console's first session player (its own peer's lowest user with a
/// slot), if any.
fn local_player(m: &dyn GuestMemory, s: u32) -> R<Option<u32>> {
    let Some(local) = local_peer(m, s)? else {
        return Ok(None);
    };
    let mask = m.u32(s + PLAYER_MASK)?;
    for u in 0..4 {
        let slot = m.u32(peer(s, local) + PEER_SLOTS + 4 * u)?;
        if slot < MAX && mask & 1 << slot != 0 {
            return Ok(Some(slot));
        }
    }
    Ok(None)
}

/// A phantom's configuration in session `s`: [`bot_config`] with the team
/// and level of this console's first session player's requested one
/// (there in the lobby, where no player datum may exist yet), else of the
/// player datum's ([`config`]). `team` overrides the host's.
pub fn config_in(
    m: &dyn GuestMemory,
    s: u32,
    name: &str,
    team: Option<u8>,
) -> R<Option<[u8; CONFIG_SIZE]>> {
    config_in_ranked(m, s, name, team, None)
}

/// [`config_in`] with the phantom's `rank` in the lobby's rank fields
/// ([`with_rank`]), for a phantom of a ranked hopper.
pub fn config_in_ranked(
    m: &dyn GuestMemory,
    s: u32,
    name: &str,
    team: Option<u8>,
    rank: Option<Rank>,
) -> R<Option<[u8; CONFIG_SIZE]>> {
    let Some(slot) = local_player(m, s)? else {
        return config(m, name, team);
    };
    let mut host = [0; CONFIG_SIZE];
    m.read(player(s, slot) + P_REQUESTED, &mut host)?;
    let mut c = from_host(&host, name, team);
    if let Some(rank) = rank {
        with_rank(&mut c, &host, rank);
    }
    Ok(Some(c))
}

/// The rank a ranked hopper's lobby row shows for a phantom, as the stats
/// read gives a person's: the level as stored (0 to 49, the shown level
/// less 1) and the experience that level starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rank {
    pub level: u8,
    pub experience: u32,
}

impl Rank {
    /// The rank of shown level `level` (1 to 50, clamped), at the
    /// experience the server's rules start it at.
    pub fn of_level(level: u8) -> Rank {
        let shown = level.clamp(live_proto::rank::MIN_LEVEL, live_proto::rank::MAX_LEVEL);
        Rank {
            level: shown - 1,
            experience: live_proto::rank::threshold(shown),
        }
    }
}

/// The configuration offset of the lobby's hopper index (0 to 15).
const CONFIG_HOPPER: usize = 0x84;
/// The two `i16`s after it: the best level (`+0x88`) and the level
/// (`+0x8a`); then the experience dword (`+0x8c`)
/// (docs/live/live-matchmaking.md §8.1, "Into the lobby").
const CONFIG_BEST_LEVEL: usize = 0x88;
const CONFIG_RANK_LEVEL: usize = 0x8a;
const CONFIG_EXPERIENCE: usize = 0x8c;

/// Puts `rank` in `c`'s host-side fields (`+0x84..+0x90`), under the
/// hopper index the host's configuration `host` carries. They are all -1
/// (no rank shown) when the index is out of range (`0x14bc80`), so a host
/// with none leaves them as they were.
fn with_rank(c: &mut [u8; CONFIG_SIZE], host: &[u8; CONFIG_SIZE], rank: Rank) {
    let index = u32::from_le_bytes(host[CONFIG_HOPPER..CONFIG_HOPPER + 4].try_into().unwrap());
    if index >= 16 {
        return;
    }
    c[CONFIG_HOPPER..CONFIG_HOPPER + 4].copy_from_slice(&index.to_le_bytes());
    let level = i16::from(rank.level).to_le_bytes();
    c[CONFIG_BEST_LEVEL..CONFIG_BEST_LEVEL + 2].copy_from_slice(&level);
    c[CONFIG_RANK_LEVEL..CONFIG_RANK_LEVEL + 2].copy_from_slice(&level);
    c[CONFIG_EXPERIENCE..CONFIG_EXPERIENCE + 4].copy_from_slice(&rank.experience.to_le_bytes());
}

/// A sim with no local profile starts the requested configuration with its
/// own marked name and appearance. Team −1 is unaffiliated in free-for-all.
pub(super) fn playerless_config(name: &str, team: Option<u8>) -> [u8; CONFIG_SIZE] {
    bot_config(name, team.unwrap_or(u8::MAX), 0)
}

/// A configuration as `0x1537f0` clears one before a console's profile
/// fills it (`0x18fd94`): zero, and -1 in `+0x7e`, `+0x7f` and
/// `+0x84..+0x90`, the fields the host's configuration pass (`0x65cb0`)
/// keeps or assigns. A real client's requested configuration has these.
pub fn fresh_config() -> [u8; CONFIG_SIZE] {
    let mut c = [0; CONFIG_SIZE];
    c[0x7e] = 0xff;
    c[0x7f] = 0xff;
    c[0x84..].fill(0xff);
    c
}

/// A phantom's requested configuration, built as a console with a fresh
/// profile builds its own (`0x18fd94`): [`fresh_config`], the phantom's
/// name and look, `team`, no handicap, no Xbox Live team (`+0x50..+0x7c`
/// zero), the rest of the appearance block (`+0x48..+0x50`) and `+0x80`
/// zero. Only `level` (`+0x81`) is the host's: the matchmaking start
/// (`0x66050`) refuses a player whose level is below the hopper's, and the
/// host's passes it. Nothing else of the host's profile reaches a bot, so
/// a host's handicap, then or later, is never a bot's
/// (docs/game/bots.md §5.1, "A phantom's player configuration").
pub fn bot_config(name: &str, team: u8, level: u8) -> [u8; CONFIG_SIZE] {
    let mut c = fresh_config();
    set_identity(&mut c, name);
    c[CONFIG_TEAM] = team;
    c[CONFIG_HANDICAP] = HANDICAP_NONE;
    c[CONFIG_LEVEL] = level;
    c
}

/// [`bot_config`] with the host configuration `host`'s team (unless
/// `team` names one) and level.
fn from_host(host: &[u8; CONFIG_SIZE], name: &str, team: Option<u8>) -> [u8; CONFIG_SIZE] {
    bot_config(name, team.unwrap_or(host[CONFIG_TEAM]), host[CONFIG_LEVEL])
}

/// Whether the lobby's variant is a team game (`+0x4db0 + 0x48` bit 0).
pub fn team_game(m: &dyn GuestMemory, s: u32) -> R<bool> {
    Ok(m.u32(s + VARIANT + VARIANT_FLAGS)? & 1 != 0)
}

/// The most teams a configuration may name (`0x65cb0` clamps to 0..7).
const MAX_TEAMS: u8 = 8;

/// Each session player's slot, peer and effective team.
fn players_teams(m: &dyn GuestMemory, s: u32) -> R<Vec<(u32, u32, u8)>> {
    let mask = m.u32(s + PLAYER_MASK)?;
    let mut out = Vec::new();
    for slot in (0..MAX).filter(|i| mask & 1 << i != 0) {
        let a = player(s, slot);
        out.push((
            slot,
            m.u32(a + P_PEER)?,
            m.u8(a + P_EFFECTIVE + CONFIG_TEAM as u32)?,
        ));
    }
    Ok(out)
}

/// The team for a new phantom in a team game: the one with the fewest
/// players of red, blue and any other a player is on, the lowest on a tie
/// (docs/game/bots.md §5.1). The host's configuration pass (`0x65cb0`)
/// takes a requested team as it is in a team game (clamped to 0..7), so
/// the bots keep the teams even. In a Firefight lobby only the player
/// teams count ([`crate::firefight::seat_team`]): never the AI's.
pub fn lobby_team(m: &dyn GuestMemory, s: u32) -> R<u8> {
    let teams = players_teams(m, s)?;
    if let Some(n) = crate::firefight::seat_teams() {
        return Ok(crate::firefight::seat_team(teams.iter().map(|t| t.2), n));
    }
    let count = team_counts(teams.iter().map(|t| t.2));
    Ok(fewest(&count))
}

/// The players on each team: red and blue (0 and 1) always, and any
/// other team a player is on.
fn team_counts(teams: impl Iterator<Item = u8>) -> Vec<(u8, usize)> {
    let mut count: Vec<(u8, usize)> = vec![(0, 0), (1, 0)];
    for t in teams.filter(|&t| t < MAX_TEAMS) {
        match count.iter_mut().find(|c| c.0 == t) {
            Some(c) => c.1 += 1,
            None => count.push((t, 1)),
        }
    }
    count.sort_unstable();
    count
}

fn fewest(count: &[(u8, usize)]) -> u8 {
    count.iter().min_by_key(|c| (c.1, c.0)).map_or(0, |c| c.0)
}

/// Sets session player `slot`'s team, requested and effective, and bumps
/// the counters.
pub fn set_team(m: &mut dyn GuestMemory, s: u32, slot: u32, team: u8) -> R<()> {
    write_team(m, s, slot, team)?;
    bump(m, s)
}

fn write_team(m: &mut dyn GuestMemory, s: u32, slot: u32, team: u8) -> R<()> {
    let a = player(s, slot);
    m.set_u8(a + P_REQUESTED + CONFIG_TEAM as u32, team)?;
    m.set_u8(a + P_EFFECTIVE + CONFIG_TEAM as u32, team)
}

/// Keeps the teams even in a team game's lobby by moving the phantoms of
/// `movable` (peer indices: the phantoms with no team asked for): while
/// one's team has two players more than the smallest, it goes to the
/// smallest. A human changing team, or the variant turning into a team
/// game, is followed; nobody moves for a difference of one. Returns each
/// move as (peer, from, to). The membership counters move once for all of
/// them.
pub fn rebalance(m: &mut dyn GuestMemory, s: u32, movable: &[u32]) -> R<Vec<(u32, u8, u8)>> {
    let moved = rebalance_teams(m, s, movable)?;
    if !moved.is_empty() {
        bump(m, s)?;
    }
    Ok(moved)
}

/// [`rebalance`] without the membership bump ([`bump`] once after).
pub(super) fn rebalance_teams(
    m: &mut dyn GuestMemory, s: u32, movable: &[u32],
) -> R<Vec<(u32, u8, u8)>> {
    if !team_game(&*m, s)? {
        return Ok(Vec::new());
    }
    let players = players_teams(&*m, s)?;
    let mut count = team_counts(players.iter().map(|t| t.2));
    // Firefight (crate::firefight): a phantom on any other team than the
    // player teams moves onto one.
    if let Some(n) = crate::firefight::seat_teams() {
        count.retain(|c| c.0 < n);
    }
    let mut moved = Vec::new();
    for &(slot, p, team) in &players {
        if !movable.contains(&p) {
            continue;
        }
        let to = fewest(&count);
        let least = count.iter().find(|c| c.0 == to).map_or(0, |c| c.1);
        let now = count.iter().find(|c| c.0 == team).map(|c| c.1);
        // Off every team (0xff, or past the eighth): in. On one with two
        // more than the smallest: across.
        if now.is_some_and(|n| n < least + 2) {
            continue;
        }
        if let Some(c) = count.iter_mut().find(|c| c.0 == team) {
            c.1 -= 1;
        }
        if let Some(c) = count.iter_mut().find(|c| c.0 == to) {
            c.1 += 1;
        }
        write_team(m, s, slot, to)?;
        moved.push((p, team, to));
    }
    Ok(moved)
}

/// The world's state while a server's game starts ([`SIM_STATE`]).
pub const SIM_STARTING: u32 = 3;

/// The game's start stops waiting for phantom machines (docs/game/bots.md
/// §5.1, "Phantoms in the game"). A server's world, starting (state 3),
/// waits until every machine in its wait mask (`+0x20`) but its own has a
/// joined view, or until the timeout `[0x4ced4c]` (`0x69c80`); a phantom
/// has no channel, so no view, and the game waited the whole timeout
/// with phantoms in it. Their bits come out of the mask, so the start
/// waits for real consoles only. Returns the mask before and after, when
/// it changed.
pub fn unwait_phantoms(m: &mut dyn GuestMemory) -> R<Option<(u32, u32)>> {
    let sim = m.u32(SIM_GLOBALS)?;
    if sim == 0 {
        return Ok(None);
    }
    let mode = m.u32(sim + SIM_MODE)?;
    // The servers: synchronous (2) and distributed (4).
    if !matches!(mode, 2 | WORLD_DIST_SERVER) || m.u32(sim + SIM_STATE)? != SIM_STARTING {
        return Ok(None);
    }
    let set = m.u32(MACHINES)?;
    if set == 0 {
        return Ok(None);
    }
    let valid = m.u32(set + MACHINES_MASK)?;
    let wait = m.u32(sim + SIM_WAIT)?;
    let mut left = wait;
    for i in (0..MAX).filter(|i| wait & valid & 1 << i != 0) {
        let id = mac_at(&*m, set + MACHINES_IDS + 6 * i)?;
        if crate::phantom::handle_of(id).is_some() {
            left &= !(1 << i);
        }
    }
    if left == wait {
        return Ok(None);
    }
    m.set_u32(sim + SIM_WAIT, left)?;
    Ok(Some((wait, left)))
}

/// Whether this console serves a multiplayer game now: one loaded, its
/// simulation world the distributed server (world mode
/// [`WORLD_DIST_SERVER`], as [`host_session`] reads it). The game
/// options' own mode ([`guest::game_mode`]) cannot say so: it is 2 for
/// every multiplayer game, host or client.
pub fn serves_game(m: &dyn GuestMemory) -> bool {
    if guest::game_tick(m).is_none() || !multiplayer(m) {
        return false;
    }
    let Ok(sim) = m.u32(SIM_GLOBALS) else {
        return false;
    };
    sim != 0 && m.u32(sim + SIM_MODE).ok() == Some(WORLD_DIST_SERVER)
}

fn multiplayer(m: &dyn GuestMemory) -> bool {
    let Ok(g) = m.u32(points::GAME_GLOBALS.base) else {
        return false;
    };
    g != 0 && m.u32(g + guest::OPTIONS).ok() == Some(guest::MODE_MULTIPLAYER)
}

/// Moves both membership counters (`+UPDATE`, `+CHANGES`) on, as the
/// host's own membership edits do. The game rebuilds what depends on the
/// membership once per change it sees, so a frame's edits share one bump.
pub(super) fn bump(m: &mut dyn GuestMemory, s: u32) -> R<()> {
    let u = m.u32(s + UPDATE)?;
    m.set_u32(s + UPDATE, u.wrapping_add(1))?;
    let c = m.u32(s + CHANGES)?;
    m.set_u32(s + CHANGES, c.wrapping_add(1))
}

/// The players peer `p` of `s` holds (peer `+0xf8`).
pub fn peer_players(m: &dyn GuestMemory, s: u32, p: u32) -> R<u32> {
    m.u32(peer(s, p) + PEER_PLAYERS)
}

fn peer(s: u32, p: u32) -> u32 {
    s + PEERS + p * PEER_SIZE
}

fn player(s: u32, slot: u32) -> u32 {
    s + PLAYERS + slot * PLAYER_SIZE
}

fn channel(s: u32, p: u32) -> u32 {
    s + CHANNELS + p * CHANNEL_SIZE
}

fn zero(m: &mut dyn GuestMemory, a: u32, n: u32) -> R<()> {
    m.write(a, &vec![0; n as usize])
}

fn mac_at(m: &dyn GuestMemory, a: u32) -> R<[u8; 6]> {
    let mut id = [0; 6];
    m.read(a, &mut id)?;
    Ok(id)
}

pub(super) fn peer_machine(m: &dyn GuestMemory, s: u32, p: u32) -> R<[u8; 6]> {
    mac_at(m, peer(s, p) + PEER_MAC)
}

/// Whether an occupied membership slot belongs to a phantom machine.
/// This also works on clients, which do not own that phantom's brain.
pub fn phantom_player(m: &dyn GuestMemory, s: u32, id: u32) -> R<bool> {
    let slot = id & 0xffff;
    if slot >= MAX || m.u32(s + PLAYER_MASK)? & (1 << slot) == 0 {
        return Ok(false);
    }
    let p = m.u32(player(s, slot) + P_PEER)?;
    Ok(p < m.u32(s + PEER_COUNT)?.min(MAX)
        && super::handle_of(peer_machine(m, s, p)?).is_some())
}

/// This console hosts the session and has its sole human player.
/// An unclassified remote peer counts as human even before its players join.
pub fn solo_host(m: &dyn GuestMemory, s: u32) -> R<bool> {
    if m.u32(s + STATE)? != STATE_LIVE {
        return Ok(false);
    }
    let local = m.u32(s + LOCAL_PEER)?;
    let count = m.u32(s + PEER_COUNT)?;
    if count > MAX || local >= count || m.u32(s + HOST_PEER)? != local {
        return Ok(false);
    }
    for p in (0..count).filter(|&p| p != local) {
        if super::handle_of(peer_machine(m, s, p)?).is_none() {
            return Ok(false);
        }
    }
    let mask = m.u32(s + PLAYER_MASK)?;
    let mut locals = 0;
    for slot in (0..MAX).filter(|&i| mask & (1 << i) != 0) {
        let p = m.u32(player(s, slot) + P_PEER)?;
        if p >= count {
            return Ok(false);
        }
        locals += u32::from(p == local);
    }
    Ok(locals == 1)
}

/// The peer with machine id `mac`.
pub fn find_peer(m: &dyn GuestMemory, s: u32, mac: [u8; 6]) -> R<Option<u32>> {
    find_peer_near(m, s, mac, None)
}

pub(super) fn find_peer_near(
    m: &dyn GuestMemory, s: u32, mac: [u8; 6], hint: Option<u32>,
) -> R<Option<u32>> {
    let n = m.u32(s + PEER_COUNT)?.min(MAX);
    if let Some(p) = hint && p < n && peer_machine(m, s, p)? == mac {
        return Ok(Some(p));
    }
    for p in 0..n {
        if peer_machine(m, s, p)? == mac {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// A phantom's 12-byte session player id: its xuid, then its machine id's
/// last four bytes (its handle).
pub fn player_id(xuid: u64, mac: [u8; 6]) -> [u8; 12] {
    let mut id = [0; 12];
    id[..8].copy_from_slice(&xuid.to_le_bytes());
    id[8..].copy_from_slice(&mac[2..]);
    // The low two bits are a Live guest number. Each phantom is its
    // account's primary player, including in the matchmaking party summary.
    id[8] &= !3;
    id
}

/// A phantom's configuration in a running game: [`bot_config`] with the
/// phantom's name ([`marked_name`]: the bot mark, then `name`), its team
/// when it has one (else the host's: balancing waits for the lobby teams)
/// and the host's level, both read from the host's own player (`+0xd4` of
/// the first local player). `None` with no such player.
pub fn config(m: &dyn GuestMemory, name: &str, team: Option<u8>) -> R<Option<[u8; CONFIG_SIZE]>> {
    let Some(array) = guest_layout::view::players::array(m)? else {
        return Ok(None);
    };
    for (_, p) in array.live(m, 16)? {
        if p.listed(m)? && p.local_user(m)? >= 0 {
            let mut host = [0; CONFIG_SIZE];
            m.read(p.addr + PLAYER_CONFIG, &mut host)?;
            return Ok(Some(from_host(&host, name, team)));
        }
    }
    Ok(None)
}

/// The host's own player's team (its first local listed player's
/// configuration), or `None` with no such player.
pub fn host_team(m: &dyn GuestMemory) -> R<Option<u8>> {
    let Some(array) = guest_layout::view::players::array(m)? else {
        return Ok(None);
    };
    for (_, p) in array.live(m, 16)? {
        if p.listed(m)? && p.local_user(m)? >= 0 {
            return Ok(Some(m.u8(p.addr + PLAYER_CONFIG + CONFIG_TEAM as u32)?));
        }
    }
    Ok(None)
}

/// The host's team in its pregame lobby, before any player datum exists:
/// its first session player's effective team ([`host_team`] reads the
/// datum, which a game makes). `None` with no lobby this console hosts.
pub fn lobby_host_team(m: &dyn GuestMemory) -> R<Option<u8>> {
    let Ok(s) = host_lobby(m) else {
        return Ok(None);
    };
    let Some(slot) = local_player(m, s)? else {
        return Ok(None);
    };
    Ok(Some(
        m.u8(player(s, slot) + P_EFFECTIVE + CONFIG_TEAM as u32)?,
    ))
}

pub use live_proto::bots::BOT_MARK as MARK;

/// Retail display names have 32 UTF-16 units including the terminator.
pub fn marked_name(tag: &str) -> Vec<u16> {
    let mut units = vec![MARK as u16, u16::from(b' ')];
    units.extend(tag.trim_start().encode_utf16().take(CONFIG_NAME_UNITS - 3));
    units
}

// The configuration begins with the wide display name immediately followed
// by the eight appearance bytes that retail validates at 0x153750.
fn identity_bytes(name: &str) -> [u8; CONFIG_LOOK + 8] {
    let mut bytes = [0; CONFIG_LOOK + 8];
    for (dst, u) in bytes[..CONFIG_LOOK]
        .chunks_exact_mut(2)
        .zip(marked_name(name))
    {
        dst.copy_from_slice(&u.to_le_bytes());
    }
    bytes[CONFIG_LOOK..].copy_from_slice(&guest::bot_look(name));
    bytes
}

fn set_identity(c: &mut [u8; CONFIG_SIZE], name: &str) {
    c[..CONFIG_LOOK + 8].copy_from_slice(&identity_bytes(name));
}

// The host can rebuild a player's effective configuration while the phantom
// is seated. A reclaimed peer can also carry an earlier profile. Publish the
// identity in both copies without disturbing team or ready state; the
// handicap has its own repair ([`write_no_handicap`]). Returns whether it
// wrote; the caller bumps the membership once per frame ([`bump`]).
pub(super) fn write_identity(
    m: &mut dyn GuestMemory, s: u32, p: u32, identity: &PeerIdentity,
) -> R<bool> {
    let Some(a) = seated_player(m, s, p)? else {
        return Ok(false);
    };
    let want = &identity.player;
    let mut have = [0; CONFIG_LOOK + 8];
    let mut changed = false;
    for at in [a + P_REQUESTED, a + P_EFFECTIVE] {
        m.read(at, &mut have)?;
        if &have != want {
            m.write(at, want)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Peer `p`'s first session player, when its slot is in use and the
/// player names `p` as its peer.
fn seated_player(m: &dyn GuestMemory, s: u32, p: u32) -> R<Option<u32>> {
    let slot = m.u32(peer(s, p) + PEER_SLOTS)?;
    if slot >= MAX || m.u32(s + PLAYER_MASK)? & (1 << slot) == 0 {
        return Ok(None);
    }
    let a = player(s, slot);
    Ok((m.u32(a + P_PEER)? == p).then_some(a))
}

// A seat taken back from an older run, or a reclaimed peer, can carry a
// handicap a bot never asks for. Puts both copies back to none (the host's
// pass `0x65cb0` copies the requested handicap into the effective one); the
// team, the rest of the configuration and `+0x138` stay. Returns whether it
// wrote; the caller bumps the membership once per frame ([`bump`]), and the
// game then rescales the unit's shields (`0x14be90` → `0x1583d0`).
pub(super) fn write_no_handicap(m: &mut dyn GuestMemory, s: u32, p: u32) -> R<bool> {
    let Some(a) = seated_player(m, s, p)? else {
        return Ok(false);
    };
    let mut changed = false;
    for at in [a + P_REQUESTED, a + P_EFFECTIVE] {
        let h = at + CONFIG_HANDICAP as u32;
        if m.u8(h)? != HANDICAP_NONE {
            m.set_u8(h, HANDICAP_NONE)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Adds a peer for machine `mac`, unconnected and with no channel, as
/// `0x5fbd0(S, n, &xnaddr, 0, -1, NULL)` does, and bumps the membership
/// counters as the host's join (`0x5c720`) does. Returns its index.
pub fn add_peer(m: &mut dyn GuestMemory, s: u32, mac: [u8; 6], name: &str) -> R<Option<u32>> {
    let n = m.u32(s + PEER_COUNT)?;
    if n >= MAX || n >= m.u32(s + MAX_PEERS)? {
        return Ok(None);
    }
    m.set_u32(s + PEER_COUNT, n + 1)?;
    let a = peer(s, n);
    zero(m, a, PEER_SIZE)?;
    // The XNADDR: only its MAC (the machine id) is ours to set.
    m.write(a + PEER_MAC, &mac)?;
    for (i, u) in (0u32..).zip(name.encode_utf16().take(PEER_NAME_UNITS as usize)) {
        m.set_u16(a + PEER_NAME + 2 * i, u)?;
    }
    for u in 0..4 {
        m.set_u32(a + PEER_SLOTS + 4 * u, u32::MAX)?;
    }
    let c = channel(s, n);
    zero(m, c, CHANNEL_SIZE)?;
    m.set_u8(c, 1)?; // valid; +1 connected stays 0
    m.set_u32(c + 4, u32::MAX)?; // no channel
    m.set_u32(c + 8, u32::MAX)?;
    m.set_u32(c + 0xc, u32::MAX)?;
    bump(m, s)?;
    Ok(Some(n))
}

/// Adds user 0 of peer `p` as a session player, as `0x60000`/`0x600f0`
/// and the add-player message (`0x5f05d`) do: controller 0, the
/// configuration both requested and effective. Returns its slot.
pub fn add_player(
    m: &mut dyn GuestMemory,
    s: u32,
    p: u32,
    id: [u8; 12],
    config: &[u8; CONFIG_SIZE],
) -> R<Option<u32>> {
    let mask = m.u32(s + PLAYER_MASK)?;
    let count = m.u32(s + PLAYER_COUNT)?;
    if free_player_slots(&*m, s)? == 0 {
        return Ok(None);
    }
    for slot in (0..MAX).filter(|i| mask & 1 << i != 0) {
        let mut other = [0; 12];
        m.read(player(s, slot) + P_ID, &mut other)?;
        if other == id {
            return Ok(None);
        }
    }
    let Some(slot) = (0..MAX).find(|i| mask & 1 << i == 0) else {
        return Ok(None);
    };
    let a = player(s, slot);
    m.write(a + P_ID, &id)?;
    m.set_u32(a + P_PEER, p)?;
    m.set_u32(a + P_USER, 0)?;
    m.set_u32(a + P_CONTROLLER, 0)?;
    m.write(a + P_REQUESTED, config)?;
    m.write(a + P_EFFECTIVE, config)?;
    m.set_u32(a + P_138, 0)?;
    let pa = peer(s, p);
    m.set_u32(pa + PEER_SLOTS, slot)?;
    let on = m.u32(pa + PEER_PLAYERS)?;
    m.set_u32(pa + PEER_PLAYERS, on + 1)?;
    m.set_u32(s + PLAYER_MASK, mask | 1 << slot)?;
    m.set_u32(s + PLAYER_COUNT, count + 1)?;
    bump(m, s)?;
    Ok(Some(slot))
}

/// Takes peer `p` and its players out, as `0x5fe20` does: the players go
/// (`0x60200`), later players' peer indices and the session's peer
/// indices move down, and the peers and channels above `p` move down one.
pub fn remove_peer(m: &mut dyn GuestMemory, s: u32, p: u32) -> R<()> {
    let n = m.u32(s + PEER_COUNT)?.min(MAX);
    if p >= n {
        return Ok(());
    }
    let pa = peer(s, p);
    for u in 0..4 {
        let slot = m.u32(pa + PEER_SLOTS + 4 * u)?;
        if slot >= MAX {
            continue;
        }
        m.set_u32(pa + PEER_SLOTS + 4 * u, u32::MAX)?;
        let on = m.u32(pa + PEER_PLAYERS)?;
        m.set_u32(pa + PEER_PLAYERS, on.saturating_sub(1))?;
        let mask = m.u32(s + PLAYER_MASK)?;
        m.set_u32(s + PLAYER_MASK, mask & !(1 << slot))?;
        let count = m.u32(s + PLAYER_COUNT)?;
        m.set_u32(s + PLAYER_COUNT, count.saturating_sub(1))?;
        bump(m, s)?;
    }
    let mask = m.u32(s + PLAYER_MASK)?;
    for slot in (0..MAX).filter(|i| mask & 1 << i != 0) {
        let a = player(s, slot) + P_PEER;
        let q = m.u32(a)?;
        if q > p && q < MAX {
            m.set_u32(a, q - 1)?;
        }
    }
    if m.u32(s + PEER_50)? == p {
        m.set_u32(s + PEER_50, 0)?;
    }
    for (src, size) in [(PEERS, PEER_SIZE), (CHANNELS, CHANNEL_SIZE)] {
        let tail = (n - p - 1) * size;
        if tail > 0 {
            let mut buf = vec![0; tail as usize];
            m.read(s + src + (p + 1) * size, &mut buf)?;
            m.write(s + src + p * size, &buf)?;
        }
        zero(m, s + src + (n - 1) * size, size)?;
    }
    for f in [LOCAL_PEER, HOST_PEER, PEER_50] {
        let q = m.u32(s + f)?;
        if q > p && q < MAX {
            m.set_u32(s + f, q - 1)?;
        }
    }
    m.set_u32(s + PEER_COUNT, n - 1)?;
    bump(m, s)
}

/// The simulation slot of machine `mac`'s player, once the game made it.
pub fn slot_of(m: &dyn GuestMemory, mac: [u8; 6]) -> R<Option<u32>> {
    let sim = m.u32(SIM_GLOBALS)?;
    if sim == 0 {
        return Ok(None);
    }
    for i in 0..MAX {
        let a = sim + SIM_SLOTS + i * SLOT_SIZE;
        if m.u32(a)? != u32::MAX && mac_at(m, a + SLOT_MAC)? == mac {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// Player `p`'s unit's facing and current weapon, if it has a unit.
pub fn facing(m: &dyn GuestMemory, player_addr: u32) -> R<Option<([f32; 3], i8)>> {
    let p = guest_layout::view::players::Player::at(player_addr);
    let Some(unit) = p.unit(m)? else {
        return Ok(None);
    };
    let Some((_, body)) = guest_layout::view::objects::get(m, unit)? else {
        return Ok(None);
    };
    let aim = m.vec3(body.addr + guest::UNIT_AIM)?;
    let weapon = m.u8(body.addr + UNIT_WEAPON)? as i8;
    Ok(Some((aim, weapon)))
}

#[cfg(test)]
mod tests;
