//! Phantom peers (docs/game/bots.md, section 5 and stage 6): bots that are
//! remote players of a machine that does not exist, made up in memory on
//! the host.
//!
//! [`Phantoms::add`] puts a phantom on the books and returns at once. Its
//! machine and player are made later, on the game thread
//! ([`Phantoms::refresh`] finds the player once the game has made it).
//! [`Phantoms::list`] says where each one is: waiting, joined (with its
//! player's index), or failed and why.
//!
//! Each phantom has a machine id of its own ([`machine_id`]), and its
//! player is found by that id in the player records, never by the order of
//! joining.
//!
//! A phantom joins the pregame lobby this console hosts, as a real peer
//! joins it, and stays in the session into the game and back to the
//! lobby; [`LOBBY_ENV`] off keeps it to the game alone.

use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::{GuestMemory, MemFault};
use engine::view::players;

use crate::guest;
use crate::local::PadSource;

/// Who a phantom is (docs/game/bots.md, section 9.1): the server's account
/// and how the lobby shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub xuid: u64,
    pub gamertag: String,
    /// Emblem foreground, background and three colours, primary and
    /// secondary armour colours, as the roster gives them.
    pub emblem: [u8; 5],
    pub colors: [u8; 2],
}

/// The most phantoms: 16 machines, one of them the host.
pub const MAX_PHANTOMS: usize = 15;

/// A phantom on the books.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhantomHandle(pub u32);

/// The first two bytes of every phantom's machine id; the other four are
/// its handle, big-endian. No real machine id starts with them: real ones
/// are the console's Ethernet address, and `C1` has the multicast bit set,
/// which no unicast adapter address carries.
pub const MACHINE_PREFIX: [u8; 2] = [0xc1, 0xa4];

/// A phantom's 6-byte machine id.
pub fn machine_id(h: PhantomHandle) -> [u8; 6] {
    let b = h.0.to_be_bytes();
    [MACHINE_PREFIX[0], MACHINE_PREFIX[1], b[0], b[1], b[2], b[3]]
}

/// The phantom a machine id belongs to, if it is one of ours.
pub fn handle_of(machine: [u8; 6]) -> Option<PhantomHandle> {
    (machine[..2] == MACHINE_PREFIX).then(|| {
        PhantomHandle(u32::from_be_bytes([
            machine[2], machine[3], machine[4], machine[5],
        ]))
    })
}

/// Where a phantom is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// On the books; the game has not made its player yet.
    Pending,
    /// The game has a player for it.
    Joined,
    /// It could not be made, and why. It stays listed until removed.
    Failed(String),
}

/// What [`Phantoms::list`] says about one phantom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhantomInfo {
    pub handle: PhantomHandle,
    pub identity: Identity,
    /// The team asked for; `None`: the host balances.
    pub team: Option<u8>,
    pub machine: [u8; 6],
    /// Its player's datum index (`player.<i>` in `sim.view`), once joined.
    pub player: Option<u32>,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// [`MAX_PHANTOMS`] already.
    Full,
    /// A phantom with that xuid is already on the books.
    Duplicate(u64),
    /// An identity with no gamertag or a zero xuid.
    BadIdentity,
    NoSuchPhantom(PhantomHandle),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Full => write!(f, "{MAX_PHANTOMS} phantoms already"),
            Error::Duplicate(x) => write!(f, "a phantom with xuid {x:016x} already"),
            Error::BadIdentity => write!(f, "a phantom needs a gamertag and a non-zero xuid"),
            Error::NoSuchPhantom(h) => write!(f, "no phantom {}", h.0),
        }
    }
}
impl std::error::Error for Error {}

struct Phantom {
    info: PhantomInfo,
    source: Box<dyn PadSource>,
    /// Its peer is in the host's session.
    made: bool,
    buttons: action::ButtonState,
    uplink: action::Uplink,
    /// Frame-indexed sources advance once per simulation sample, across respawns.
    samples: u64,
    /// Why it waits for hosting, as last logged ([`waiting_line`]).
    waiting: Option<String>,
}

impl Phantom {
    fn reset_input(&mut self) {
        self.buttons = Default::default();
        self.uplink.clear();
        self.source.reset_input();
    }
}

/// The log line for a phantom that waits because this console is not (or
/// not yet) the game's host (`why`, [`session::NotNow::NotHost`]).
pub fn waiting_line(gamertag: &str, why: &str) -> String {
    format!("bots: phantom {gamertag} waits: this console is not the game's host yet ({why})")
}

/// The log line for a phantom that waits because the pregame lobby up is
/// not one it can join (`why`, from [`session::host_lobby`]).
pub fn lobby_waiting_line(gamertag: &str, why: &str) -> String {
    format!("bots: phantom {gamertag} waits: not a lobby it can join ({why})")
}

/// The environment switch for phantoms in the pregame lobby
/// (docs/game/bots.md §5.1, "Phantoms in the pregame lobby"): `0`, `off`
/// or `false` keeps them out of the lobby, joining once the game runs and
/// leaving when it ends, as before. On by default.
pub const LOBBY_ENV: &str = "H2_PHANTOM_LOBBY";

/// Whether [`LOBBY_ENV`]'s value leaves phantoms in the lobby.
pub fn lobby_wanted(v: Option<&str>) -> bool {
    !matches!(
        v.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

#[derive(Default)]
struct Books {
    list: Vec<Phantom>,
    hosted_game: Option<u32>,
    next: u32,
    /// Machine ids of removed phantoms whose peers are still in the
    /// session, for the game thread to take out.
    gone: Vec<[u8; 6]>,
    /// Phantoms join only once a game runs and leave when it ends (the
    /// lobby join off, [`LOBBY_ENV`]).
    game_only: bool,
    /// A Clankermatch's phantoms: they join only the match's session (the
    /// matchmaking life cycle, types 6 to 9), never the party's lobby
    /// ([`Phantoms::set_match_only`]).
    match_only: bool,
}

/// Why a Clankermatch's phantom waits outside the match.
pub const NOT_THE_MATCH: &str =
    "the party's lobby: a Clankermatch's bots join the match the search hosted, never the party";

/// Where the session a phantom can join is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// A multiplayer game this console hosts runs ([`session::host_session`]).
    Game(u32),
    /// The pregame lobby this console hosts ([`session::host_lobby`]).
    Lobby(u32),
}

/// The phantoms of one host. Cheap to clone; clones share.
#[derive(Clone, Default)]
pub struct Phantoms(Arc<Mutex<Books>>);

impl Phantoms {
    fn lock(&self) -> std::sync::MutexGuard<'_, Books> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Puts a phantom on the books, [`State::Pending`]. Never waits on the
    /// game.
    pub fn add(
        &self,
        identity: Identity,
        team: Option<u8>,
        source: Box<dyn PadSource>,
    ) -> Result<PhantomHandle, Error> {
        if identity.xuid == 0 || identity.gamertag.trim().is_empty() {
            return Err(Error::BadIdentity);
        }
        let mut b = self.lock();
        if b.list.iter().any(|p| p.info.identity.xuid == identity.xuid) {
            return Err(Error::Duplicate(identity.xuid));
        }
        if b.list.len() >= MAX_PHANTOMS {
            return Err(Error::Full);
        }
        let handle = PhantomHandle(b.next);
        b.next += 1;
        let uplink = action::Uplink::new(identity.xuid);
        b.list.push(Phantom {
            info: PhantomInfo {
                handle,
                identity,
                team,
                machine: machine_id(handle),
                player: None,
                state: State::Pending,
            },
            source,
            made: false,
            buttons: Default::default(),
            uplink,
            samples: 0,
            waiting: None,
        });
        Ok(handle)
    }

    /// Whether `machine` is a phantom of these books with a peer in the
    /// session: the game's "is this machine in the game" (`0x686e0`) is
    /// answered yes for it (docs/game/bots.md §5.1, "Phantoms in the
    /// game"), since it has no simulation view to say so.
    pub fn stands_for(&self, machine: [u8; 6]) -> bool {
        handle_of(machine).is_some()
            && self
                .lock()
                .list
                .iter()
                .any(|p| p.made && p.info.machine == machine)
    }

    /// Whether phantoms join the pregame lobby (the default), or only a
    /// running game ([`LOBBY_ENV`]).
    pub fn set_lobby(&self, on: bool) {
        self.lock().game_only = !on;
    }

    /// Whether the phantoms are a Clankermatch's (docs/game/bots.md §8.4,
    /// "Phantoms only"): they join the session the search hosted and its
    /// game (life cycle 6 to 9, [`crate::lobby::matchmaking_cycle`]), and
    /// leave any other, the party's lobby before the search and after the
    /// game above all. Set by the Clankermatch fill while it runs.
    pub fn set_match_only(&self, on: bool) {
        self.lock().match_only = on;
    }

    /// Whether the phantoms are a Clankermatch's ([`Phantoms::set_match_only`]).
    pub fn match_only(&self) -> bool {
        self.lock().match_only
    }

    /// Takes a phantom off the books.
    pub fn remove(&self, handle: PhantomHandle) -> Result<(), Error> {
        let mut b = self.lock();
        let i = b
            .list
            .iter()
            .position(|p| p.info.handle == handle)
            .ok_or(Error::NoSuchPhantom(handle))?;
        let p = b.list.remove(i);
        if p.made {
            b.gone.push(p.info.machine);
        }
        Ok(())
    }

    /// No phantoms on the books and none left to take out of a session.
    pub fn is_empty(&self) -> bool {
        let b = self.lock();
        b.list.is_empty() && b.gone.is_empty()
    }

    pub fn list(&self) -> Vec<PhantomInfo> {
        self.lock().list.iter().map(|p| p.info.clone()).collect()
    }

    /// Phantom `handle`'s player index, once joined.
    pub fn player(&self, handle: PhantomHandle) -> Option<u32> {
        self.lock()
            .list
            .iter()
            .find(|p| p.info.handle == handle)
            .and_then(|p| p.info.player)
    }

    pub fn brain_view(&self, handle: PhantomHandle) -> Option<crate::BrainView> {
        self.lock()
            .list
            .iter()
            .find(|p| p.info.handle == handle)?
            .source
            .brain_view()
    }

    /// Marks a phantom failed (the game thread could not make it).
    pub fn fail(&self, handle: PhantomHandle, why: impl Into<String>) {
        if let Some(p) = self
            .lock()
            .list
            .iter_mut()
            .find(|p| p.info.handle == handle)
        {
            p.info.state = State::Failed(why.into());
            p.info.player = None;
        }
    }

    /// Finds each phantom's player in the player records by its machine
    /// id. A phantom whose player has gone goes back to pending; a failed
    /// one stays failed.
    pub fn refresh(&self, m: &dyn GuestMemory) -> Result<(), MemFault> {
        self.refresh_where(m, false)
    }

    /// [`Phantoms::refresh`]; with `only_made`, a phantom with no peer in
    /// the session is pending whatever the player records still hold (they
    /// outlive a game).
    fn refresh_where(&self, m: &dyn GuestMemory, only_made: bool) -> Result<(), MemFault> {
        let found = players_by_machine(m)?;
        for p in &mut self.lock().list {
            if matches!(p.info.state, State::Failed(_)) {
                continue;
            }
            if only_made && !p.made {
                p.info.player = None;
                p.info.state = State::Pending;
                continue;
            }
            let at = found
                .iter()
                .find(|(id, _)| *id == p.info.machine)
                .map(|f| f.1);
            p.info.player = at;
            p.info.state = if at.is_some() {
                State::Joined
            } else {
                State::Pending
            };
        }
        Ok(())
    }
}

impl Phantoms {
    /// One frame on the game thread (docs/game/bots.md §5.1). While this
    /// console hosts a pregame lobby (System Link or Xbox Live), or a
    /// multiplayer game it hosts runs, pending phantoms get a peer, its
    /// properties and a session player, as a real peer joining does: in
    /// the lobby they show in its roster and the game takes them in when
    /// it starts. The simulation samples each joined phantom's pad when it
    /// gathers actions (`slot_action`). Removed phantoms' peers come out. When
    /// this console hosts no session any more, every phantom's peer comes
    /// out and the phantom waits for the next. Returns lines for the log.
    pub fn tick(&self, m: &mut dyn GuestMemory, _frame: u64) -> Result<Vec<String>, MemFault> {
        let mut log = Vec::new();
        // A starting game waits for no phantom (§5.1, "Phantoms in the
        // game"), whichever way they came in.
        if let Some((was, now)) = session::unwait_phantoms(m)? {
            log.push(format!(
                "bots: phantoms: the game's start waits for real consoles only \
                 (machines {was:#06x} -> {now:#06x})"
            ));
        }
        let mut b = self.lock();
        // A Clankermatch's phantoms stay out of the party (§8.4).
        let barred = b.match_only && !crate::lobby::matchmaking_cycle(&*m);
        let game = if barred {
            Err(session::NotNow::Wait)
        } else {
            session::host_session(&*m)
        };
        let lobby = match &game {
            Ok(_) => Err(session::NotNow::Wait),
            Err(_) if barred => Err(session::NotNow::NotHost(NOT_THE_MATCH.into())),
            Err(_) if b.game_only => Err(session::NotNow::Wait),
            Err(_) => session::host_lobby(&*m),
        };
        let phase = match (&game, &lobby) {
            (Ok(s), _) => Some(Phase::Game(*s)),
            (_, Ok(s)) => Some(Phase::Lobby(*s)),
            _ => None,
        };
        let hosted_game = match phase {
            Some(Phase::Game(session)) => Some(session),
            _ => None,
        };
        if b.hosted_game.is_some() && b.hosted_game != hosted_game {
            for p in &mut b.list {
                p.reset_input();
            }
        }
        b.hosted_game = hosted_game;
        let live = session::live_session(&*m);

        // Out: removed phantoms, and all of them once this console hosts
        // no session (with the lobby join off: once no game runs).
        let mut out: Vec<[u8; 6]> = std::mem::take(&mut b.gone);
        let keep =
            phase.is_some() || (!b.game_only && !barred && session::owned_session(&*m).is_some());
        if !keep {
            for p in &mut b.list {
                if p.made {
                    out.push(p.info.machine);
                    p.made = false;
                }
            }
        }
        if let Some(s) = live {
            let mut peers = Vec::new();
            for mac in &out {
                if let Some(i) = session::find_peer(&*m, s, *mac)? {
                    peers.push(i);
                }
            }
            // Last first, so the indices still to go stay put.
            peers.sort_unstable_by(|a, b| b.cmp(a));
            for i in peers {
                session::remove_peer(m, s, i)?;
            }
        }

        let Some(phase) = phase else {
            // Not the host, or not yet (the first frames of a game, a
            // lobby this console joined): the phantoms wait and try again
            // each frame, said once per phantom and reason. No game and no
            // lobby: a reason said before is said again next time.
            let why = match (game, lobby) {
                (Err(session::NotNow::NotHost(w)), _) => Some((false, w)),
                (_, Err(session::NotNow::NotHost(w))) => Some((true, w)),
                _ => None,
            };
            for p in &mut b.list {
                let Some((lobby, why)) = &why else {
                    p.waiting = None;
                    continue;
                };
                if p.info.state == State::Pending
                    && !p.made
                    && p.waiting.as_deref() != Some(why.as_str())
                {
                    let tag = &p.info.identity.gamertag;
                    log.push(if *lobby {
                        lobby_waiting_line(tag, why)
                    } else {
                        waiting_line(tag, why)
                    });
                    p.waiting = Some(why.clone());
                }
            }
            drop(b);
            self.refresh_where(&*m, true)?;
            return Ok(log);
        };
        let (s, in_lobby) = match phase {
            Phase::Game(s) => (s, false),
            Phase::Lobby(s) => (s, true),
        };

        // Kept: a made phantom's peer the game took out (its own removal)
        // joins again; the others' properties follow this console's.
        for p in &mut b.list {
            if !p.made {
                continue;
            }
            match session::find_peer(&*m, s, p.info.machine)? {
                Some(peer) => {
                    session::sync_props(m, s, peer, &p.info.identity.gamertag)?;
                }
                None => {
                    p.made = false;
                    p.reset_input();
                    log.push(format!(
                        "bots: phantom {} is no longer in the session: it joins again",
                        p.info.identity.gamertag
                    ));
                }
            }
        }

        // In: pending phantoms with no peer yet.
        let team_game = session::team_game(&*m, s)?;
        for p in &mut b.list {
            if p.made || matches!(p.info.state, State::Failed(_)) {
                continue;
            }
            let team = match p.info.team {
                Some(t) => Some(t),
                None if team_game => Some(session::lobby_team(&*m, s)?),
                None => None,
            };
            let Some(config) = session::config_in(&*m, s, &p.info.identity.gamertag, team)? else {
                continue; // no local player to copy yet
            };
            let mac = p.info.machine;
            // A peer of its own still in the session: a session that was
            // not live a while (state 7 on the user's Halo 1 run) left the
            // phantoms unmade but could not take their peers out. With its
            // player it is taken back as it is. Removing it and adding it
            // again at the end moved every later peer down and gave each
            // phantom the last index ("joins (peer 4)" four times). The
            // machine id is the phantom's own (`machine_id` of its handle,
            // roster or host-made alike), so no other phantom's peer is
            // ever found here.
            if let Some(stale) = session::find_peer(&*m, s, mac)? {
                if session::peer_players(&*m, s, stale)? > 0 {
                    session::sync_props(m, s, stale, &p.info.identity.gamertag)?;
                    p.made = true;
                    p.waiting = None;
                    log.push(format!(
                        "bots: phantom {} is still in the session (peer {stale}): it stays",
                        p.info.identity.gamertag
                    ));
                    continue;
                }
                // A peer with no player left: out, so it is never in twice.
                session::remove_peer(m, s, stale)?;
            }
            let Some(peer) = session::add_peer(m, s, mac, &p.info.identity.gamertag)? else {
                p.info.state = State::Failed("the session has no room for another machine".into());
                continue;
            };
            session::sync_props(m, s, peer, &p.info.identity.gamertag)?;
            let id = session::player_id(p.info.identity.xuid, mac);
            if session::add_player(m, s, peer, id, &config)?.is_none() {
                session::remove_peer(m, s, peer)?;
                p.info.state = State::Failed("the session has no room for another player".into());
                continue;
            }
            p.made = true;
            p.waiting = None;
            log.push(if in_lobby {
                format!(
                    "bots: phantom {} joins (peer {peer}, in the pregame lobby)",
                    p.info.identity.gamertag
                )
            } else {
                format!(
                    "bots: phantom {} joins (peer {peer})",
                    p.info.identity.gamertag
                )
            });
        }

        // Even teams in the lobby: the phantoms with no team asked for
        // follow the humans' moves.
        if in_lobby {
            let mut movable = Vec::new();
            for p in b.list.iter().filter(|p| p.made && p.info.team.is_none()) {
                if let Some(i) = session::find_peer(&*m, s, p.info.machine)? {
                    movable.push((i, p.info.identity.gamertag.clone()));
                }
            }
            let peers: Vec<u32> = movable.iter().map(|m| m.0).collect();
            for (peer, from, to) in session::rebalance(m, s, &peers)? {
                let name = movable
                    .iter()
                    .find(|x| x.0 == peer)
                    .map_or("?", |x| x.1.as_str());
                log.push(format!(
                    "bots: phantom {name} changes team {from} -> {to} to keep the teams even"
                ));
            }
        }
        drop(b);
        self.refresh_where(&*m, true)?;
        Ok(log)
    }
}

/// Every listed player's machine id and datum index.
pub fn players_by_machine(m: &dyn GuestMemory) -> Result<Vec<([u8; 6], u32)>, MemFault> {
    let Some(array) = players::array(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (handle, p) in array.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        out.push((guest::player_machine(m, p.addr)?, u32::from(handle.index())));
    }
    Ok(out)
}

/// The process-wide phantoms the boot glue drives.
pub fn global() -> &'static Phantoms {
    static G: OnceLock<Phantoms> = OnceLock::new();
    G.get_or_init(|| {
        let p = Phantoms::default();
        p.set_lobby(lobby_wanted(std::env::var(LOBBY_ENV).ok().as_deref()));
        p
    })
}

/// Adds a phantom peer with `identity` on `team` (`None`: the host
/// balances), driven by `source` (see [`Phantoms::add`]). It joins the
/// pregame lobby this console hosts (System Link or Xbox Live), or a
/// multiplayer game it hosts once that runs.
pub fn add(
    identity: Identity,
    team: Option<u8>,
    source: Box<dyn PadSource>,
) -> Result<PhantomHandle, Error> {
    global().add(identity, team, source)
}

/// Removes a phantom peer.
pub fn remove(handle: PhantomHandle) -> Result<(), Error> {
    global().remove(handle)
}

/// The phantoms on the books.
pub fn list() -> Vec<PhantomInfo> {
    global().list()
}

/// Phantom `handle`'s player index, once joined (`sim`'s
/// `phantom_player`).
pub fn player(handle: PhantomHandle) -> Option<u32> {
    global().player(handle)
}

mod action;
pub mod session;

#[cfg(test)]
mod tests;
