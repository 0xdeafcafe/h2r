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

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::players;

use crate::guest;
use crate::pad_source::PadSource;
use crate::think::{Mode, Source, Thinker, lock_source};

/// Who a phantom is (docs/game/bots.md, section 9.1): the server's account
/// and how the lobby shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub xuid: u64,
    pub gamertag: String,
}

/// The most phantoms: 16 machines, one of them the host.
pub const MAX_PHANTOMS: usize = 15;

/// A phantom on the books.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum State {
    /// On the books; the game has not made its player yet.
    Pending,
    /// The game has a player for it.
    Joined,
    /// It could not be made, and why. It stays listed until removed.
    Failed(String),
}

/// What [`Phantoms::list`] says about one phantom.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    peer_identity: session::PeerIdentity,
    peer_hint: Option<u32>,
    info: PhantomInfo,
    /// Shared with a think in flight (`crate::think`).
    source: Source,
    /// Bumped by every input reset, so a think from before it is dropped.
    generation: u64,
    source_reset_due: bool,
    /// Its peer is in the host's session.
    made: bool,
    buttons: action::GatherState,
    uplink: action::Uplink,
    /// Latest sampled brain decision. A brain may have no current target.
    shot_range: Option<Option<f32>>,
    /// Frame-indexed sources advance once per simulation sample, across respawns.
    samples: u64,
    /// Why it waits for hosting, as last logged ([`waiting_line`]).
    waiting: Option<String>,
    /// The rank its lobby row shows, when its hopper is a ranked one.
    rank: Option<session::Rank>,
}

impl Phantom {
    /// A render-frame transition can interrupt a worker in this source's
    /// think method. Reset the owned stream now, the source at its next gather.
    fn invalidate_input(&mut self) {
        self.buttons = Default::default();
        self.uplink.clear();
        self.shot_range = None;
        self.generation = self.generation.wrapping_add(1);
        self.source_reset_due = true;
    }

    fn reset_source_if_due(&mut self) {
        if std::mem::take(&mut self.source_reset_due) {
            lock_source(&self.source).reset_input();
        }
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
    lobby_membership: session::lobby_membership::LobbyMembership,
    maintenance: Maintenance,
    list: Vec<Phantom>,
    hosted_game: Option<u32>,
    /// Last sampled game tick and whether the opening fade was still active.
    opening_fade: Option<(u32, bool)>,
    /// A headless sim that deliberately has no local player configuration to copy.
    playerless_host: bool,
    next: u32,
    /// Machine ids of removed phantoms whose peers are still in the
    /// session, for the game thread to take out.
    gone: Vec<[u8; 6]>,
    /// Sessions this console is leaving, keyed by address and incarnation.
    /// A matchmaking cancellation can take several frames to clear its lobby.
    retiring: Vec<(u32, u32)>,
    /// How pads are sampled (`crate::think`, docs/game/bots.md §5.5).
    thinker: Thinker,
    /// The batch sampled at a tick of a hosted game: each phantom's pad.
    batch: Option<Batch>,
    /// The object scan of the tick being sampled, shared by its phantoms
    /// (`arms::share_ground`), keyed by session and tick.
    ground: crate::arms::SharedGround,
    /// Phantoms join only once a game runs and leave when it ends (the
    /// lobby join off, [`LOBBY_ENV`]).
    game_only: bool,
    /// Consecutive frames whose maintenance wrote the membership, and
    /// whether [`churn_line`] was said for this run of them.
    write_streak: u32,
    churn_said: bool,
    /// A Clankermatch's phantoms: they join only the match's session (the
    /// matchmaking life cycle, types 6 to 9), never the party's lobby
    /// ([`Phantoms::set_match_only`]).
    match_only: bool,
}

/// Roster maintenance from the render-frame hook. Counts and wall times accumulate
/// independently of combat decisions (which do not run in a lobby).
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Maintenance {
    pub frames: u64,
    pub unchanged_lobby_frames: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    /// Frames on which the maintenance wrote the session's membership and
    /// moved its counters (a join, a removal, or a repair below). The game
    /// rebuilds what depends on the membership after each such frame.
    pub write_frames: u64,
    /// Phantom peer properties rewritten to follow this console's.
    pub props_writes: u64,
    /// Phantom names or looks written back into the session's player records.
    pub identity_writes: u64,
    /// Phantom handicaps put back to none in the session's player records.
    pub handicap_writes: u64,
    /// Phantoms moved to keep the lobby's teams even.
    pub team_moves: u64,
    /// The 8-byte chunks of the `0xc8` peer properties that differed on any
    /// properties rewrite (bit `i`: bytes `8i..8i + 8`).
    pub props_diff_chunks: u32,
    /// Built actions after which the action path cleared the phantom
    /// biped's facing-set flag, as retail's control update does for a local
    /// player ([`guest::BIPED_FACING_SET`]; counted on the action gather,
    /// not the render frame).
    pub facing_releases: u64,
}

/// Consecutive membership-writing frames after which the log says the game
/// keeps undoing a phantom write ([`churn_line`]).
pub const CHURN_FRAMES: u32 = 30;

/// The log line for [`CHURN_FRAMES`] membership writes in a row: the game
/// rewrites what the phantoms repair each frame, and every repair makes it
/// rebuild what depends on the membership.
pub fn churn_line(frames: u32, m: &Maintenance) -> String {
    format!(
        "bots: phantoms: the session's membership was rewritten on {frames} frames in a row \
         (totals: properties {}, identities {}, handicaps {}, team moves {}; property chunks {:#x})",
        m.props_writes, m.identity_writes, m.handicap_writes, m.team_moves, m.props_diff_chunks
    )
}

/// One tick's batched pads ([`Mode::Batched`]).
struct Batch {
    session: u32,
    tick: u32,
    pads: Vec<(PhantomHandle, crate::think::Report)>,
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
pub struct Phantoms(Arc<Mutex<Books>>, Arc<std::sync::atomic::AtomicU64>);

impl Phantoms {
    fn lock(&self) -> std::sync::MutexGuard<'_, Books> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn roster_revision(&self) -> u64 {
        self.1.load(std::sync::atomic::Ordering::Acquire)
    }
    fn changed(&self) {
        self.1.fetch_add(1, std::sync::atomic::Ordering::Release);
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
            peer_identity: session::PeerIdentity::new(&identity.gamertag),
            peer_hint: None,
            info: PhantomInfo {
                handle,
                identity,
                team,
                machine: machine_id(handle),
                player: None,
                state: State::Pending,
            },
            source: std::sync::Arc::new(Mutex::new(source)),
            generation: 0,
            source_reset_due: false,
            made: false,
            buttons: Default::default(),
            uplink,
            shot_range: None,
            samples: 0,
            waiting: None,
            rank: None,
        });
        self.changed();
        Ok(handle)
    }

    /// Sets the rank `handle`'s lobby row shows (`None`: none). Read when
    /// the phantom joins, so set it right after [`Phantoms::add`].
    pub fn set_rank(&self, handle: PhantomHandle, rank: Option<session::Rank>) -> Result<(), Error> {
        let mut b = self.lock();
        let p = b
            .list
            .iter_mut()
            .find(|p| p.info.handle == handle)
            .ok_or(Error::NoSuchPhantom(handle))?;
        p.rank = rank;
        Ok(())
    }

    /// The rank `handle`'s lobby row shows, if it has one.
    pub fn rank_of(&self, handle: PhantomHandle) -> Option<session::Rank> {
        self.lock()
            .list
            .iter()
            .find(|p| p.info.handle == handle)
            .and_then(|p| p.rank)
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
        let mut books = self.lock();
        if books.game_only != (!on) {
            books.game_only = !on;
            self.changed();
        }
    }

    /// Whether the phantoms are a Clankermatch's (docs/game/bots.md §8.4,
    /// "Phantoms only"): they join the session the search hosted and its
    /// game (life cycle 6 to 9, [`crate::lobby::matchmaking_cycle`]), and
    /// leave any other, the party's lobby before the search and after the
    /// game above all. Set by the Clankermatch fill while it runs.
    pub fn set_match_only(&self, on: bool) {
        let mut books = self.lock();
        if books.match_only != (on) {
            books.match_only = on;
            self.changed();
        }
    }

    /// Use each bot's identity and team to form its configuration in a playerless sim.
    pub fn set_playerless_host(&self, playerless: bool) {
        let mut books = self.lock();
        if books.playerless_host != (playerless) {
            books.playerless_host = playerless;
            self.changed();
        }
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
        self.changed();
        if p.made {
            b.gone.push(p.info.machine);
        }
        Ok(())
    }

    /// Retires this solo host's phantom peers before retail leaves the session.
    /// Configured bots can join a future session once this one is torn down.
    pub fn retire_solo_session(&self, m: &mut dyn GuestMemory, s: u32) -> Result<(), MemFault> {
        use session::at::*;
        if !session::solo_host(m, s)? {
            return Ok(());
        }
        let id = (s, m.u32(s + INCARNATION)?);
        let local = m.u32(s + LOCAL_PEER)?;
        let n = m.u32(s + PEER_COUNT)?;
        let mut b = self.lock();
        b.retiring.retain(|&(addr, _)| addr != s);
        b.retiring.push(id);
        for p in (0..n).rev().filter(|&p| p != local) {
            let machine = session::peer_machine(m, s, p)?;
            // Descending order preserves the original indices still to visit.
            session::remove_peer(m, s, p)?;
            b.gone.retain(|&mac| mac != machine);
            if let Some(phantom) = b.list.iter_mut().find(|p| p.info.machine == machine) {
                phantom.made = false;
                phantom.info.player = None;
                if phantom.info.state == State::Joined {
                    phantom.info.state = State::Pending;
                }
                phantom.invalidate_input();
                self.changed();
            }
        }
        if crate::lobby::matchmaking_cycle(m)
            && m.u32(crate::lobby::TARGET_SESSION).ok() == Some(s)
        {
            session::match_roster::sync(m, s, false)?;
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

    // Waiting for a worker to finish must leave the other seats free to gather.
    fn source(&self, handle: PhantomHandle) -> Option<Source> {
        let mut books = self.lock();
        if !books.thinker.ready_for_game() {
            return None;
        }
        let seat = books.list.iter().find(|p| p.info.handle == handle)?;
        Some(Arc::clone(&seat.source))
    }

    pub fn brain_view(&self, handle: PhantomHandle) -> Option<crate::BrainView> {
        lock_source(&self.source(handle)?).brain_view()
    }

    /// Asks phantom `handle`'s brain for a bounce route to `to`
    /// (`crate::bounce_route`); `false` for a source that takes no orders.
    pub fn order_bounce(
        &self,
        handle: PhantomHandle,
        to: [f32; 3],
        mode: crate::bounce_route::Mode,
    ) -> bool {
        self.source(handle)
            .is_some_and(|s| lock_source(&s).order_bounce(to, mode))
    }

    /// Where phantom `handle`'s bounce route stands.
    pub fn order_view(&self, handle: PhantomHandle) -> Option<crate::mind::OrderView> {
        lock_source(&self.source(handle)?).order_view()
    }

    /// Gives the joined phantom whose player is `player` an outside order
    /// (docs/game/rules-world-api.md, "Bots"), or frees it (`None`);
    /// `false` when no phantom has the seat or its source takes no orders.
    pub fn order_player(&self, player: u32, order: Option<crate::brain::Order>) -> bool {
        let handle = self
            .lock()
            .list
            .iter()
            .find(|p| p.made && p.info.player == Some(player))
            .map(|p| p.info.handle);
        let Some(handle) = handle else {
            return false;
        };
        self.source(handle).is_some_and(|s| lock_source(&s).order(order))
    }

    /// Holds phantom `handle`'s pad at `pad` (`None` lets go); `false` for
    /// a source that cannot be held or a seat not there.
    pub fn hold_pad(&self, handle: PhantomHandle, pad: Option<input::host::HostPadState>) -> bool {
        self.source(handle)
            .is_some_and(|s| lock_source(&s).hold_pad(pad))
    }

    pub fn combat_stats(&self, handle: PhantomHandle) -> Option<crate::stats::CombatStats> {
        lock_source(&self.source(handle)?).combat_stats().cloned()
    }

    /// Read the current combat sample and its difficulty/style together.
    pub fn combat_state(
        &self,
        handle: PhantomHandle,
    ) -> Option<(crate::stats::CombatStats, crate::BrainView)> {
        let source = self.source(handle)?;
        let brain = lock_source(&source);
        Some((brain.combat_stats()?.clone(), brain.brain_view()?))
    }

    /// The visible target range at a player's latest combat decision.
    pub fn shot_context(&self, player: u32) -> Option<(PhantomHandle, Option<f32>)> {
        let books = self.lock();
        let seat = books.list.iter().find(|p| p.made && p.info.player == Some(player))?;
        Some((seat.info.handle, seat.shot_range?))
    }

    /// How phantom pads are sampled ([`crate::think::Mode`]): serial by
    /// default, or [`crate::think::THREADS_ENV`] and
    /// [`crate::think::PIPELINE_ENV`] for [`global`]. A think in flight
    /// finishes first.
    pub fn set_think_mode(&self, mode: Mode) {
        let mut b = self.lock();
        b.batch = None;
        b.thinker.set_mode(mode);
    }

    pub fn think_mode(&self) -> Mode {
        self.lock().thinker.mode()
    }

    /// Marks a phantom failed (the game thread could not make it).
    pub fn fail(&self, handle: PhantomHandle, why: impl Into<String>) {
        if let Some(p) = self
            .lock()
            .list
            .iter_mut()
            .find(|p| p.info.handle == handle)
        {
            self.changed();
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
        let start = std::time::Instant::now();
        let mut wrote = false;
        let mut result = self.maintain(m, &mut wrote);
        let ns = start.elapsed().as_nanos() as u64;
        let mut books = self.lock();
        let books = &mut *books;
        if result.is_err() {
            books.lobby_membership.invalidate();
        }
        let perf = &mut books.maintenance;
        perf.frames += 1;
        perf.total_ns = perf.total_ns.saturating_add(ns);
        perf.max_ns = perf.max_ns.max(ns);
        if wrote {
            perf.write_frames += 1;
            books.write_streak = books.write_streak.saturating_add(1);
            if books.write_streak >= CHURN_FRAMES && !books.churn_said {
                books.churn_said = true;
                let line = churn_line(books.write_streak, perf);
                if let Ok(log) = &mut result {
                    log.push(line);
                }
            }
        } else {
            books.write_streak = 0;
            books.churn_said = false;
        }
        result
    }

    pub fn maintenance(&self) -> Maintenance {
        self.lock().maintenance
    }

    /// `wrote`: set when this frame wrote the session's membership.
    fn maintain(&self, m: &mut dyn GuestMemory, wrote: &mut bool) -> Result<Vec<String>, MemFault> {
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
        let live = session::live_session(&*m);
        b.retiring.retain(|&(s, incarnation)| {
            use session::at::*;
            s.checked_add(INCARNATION).is_some_and(|at| {
                m.u32(s + STATE).ok() == Some(session::STATE_LIVE)
                    && m.u32(at).ok() == Some(incarnation)
            })
        });
        let retiring = b.retiring.iter().any(|&(s, _)| live == Some(s));
        // A Clankermatch's phantoms stay out of the party (§8.4).
        let barred = b.match_only && !crate::lobby::matchmaking_cycle(&*m);
        let game = if barred || retiring {
            Err(session::NotNow::Wait)
        } else {
            session::host_session(&*m)
        };
        let lobby = match &game {
            Ok(_) => Err(session::NotNow::Wait),
            Err(_) if retiring => Err(session::NotNow::Wait),
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
        if b.hosted_game != hosted_game {
            b.opening_fade = None;
            b.batch = None;
            if b.hosted_game.is_some() {
                for p in &mut b.list {
                    p.invalidate_input();
                }
            }
            // Pipelined thinks for the game that ended say nothing more.
            b.thinker.end_game();
        }
        b.hosted_game = hosted_game;

        // A menu or lobby has no current game player data. A previous map's
        // player table can stay mapped through the post-game screen.
        if hosted_game.is_none() {
            for seat in &mut b.list {
                seat.info.player = None;
                if seat.info.state == State::Joined {
                    seat.info.state = State::Pending;
                }
            }
        }
        // Inspect the actual peer and player records. A host-side profile edit
        // can leave the revision word alone, and an incarnation can reuse the
        // same guest address. Pending joins must keep trying even if a host
        // profile outside the session is all that changes.
        let settled = b.gone.is_empty() && b.list.iter().all(|p| p.made);
        let custom = matches!(phase, Some(Phase::Lobby(_)))
            && m.u32(crate::lobby::SESSION_TYPE).is_ok_and(|t| matches!(t, 1 | 2));
        if let Some(Phase::Lobby(s)) = phase
            && custom && settled
        {
            if b.lobby_membership.unchanged(&*m, s, self.roster_revision()) {
                b.maintenance.unchanged_lobby_frames += 1;
                return Ok(log);
            }
        } else {
            b.lobby_membership.invalidate();
        }

        // Out: removed phantoms, and all of them once this console hosts
        // no session (with the lobby join off: once no game runs).
        let mut out: Vec<[u8; 6]> = std::mem::take(&mut b.gone);
        let keep = phase.is_some()
            || (!b.game_only && !barred && !retiring && session::owned_session(&*m).is_some());
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
                *wrote = true;
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
            return Ok(log);
        };
        let (s, in_lobby) = match phase {
            Phase::Game(s) => (s, false),
            Phase::Lobby(s) => (s, true),
        };

        let revision_before = if custom && settled {
            Some(m.u32(s + session::at::UPDATE)?)
        } else {
            None
        };

        let host = session::host_properties(&*m, s)?;
        // Repairs of seated phantoms' records share one membership bump at
        // the end of the frame: the game rebuilds what depends on the
        // membership once per change it sees, not once per phantom.
        let mut repairs = Repairs::default();
        // Kept: a made phantom's peer the game took out (its own removal)
        // joins again; the others' properties follow this console's.
        for p in &mut b.list {
            if !p.made {
                continue;
            }
            match session::find_peer_near(&*m, s, p.info.machine, p.peer_hint)? {
                Some(peer) => {
                    p.peer_hint = Some(peer);
                    repairs.props(session::write_peer(m, s, peer, host.as_ref(), &p.peer_identity)?);
                    repairs.identity(session::write_identity(m, s, peer, &p.peer_identity)?);
                    repairs.handicap(session::write_no_handicap(m, s, peer)?);
                }
                None => {
                    p.made = false;
                    p.invalidate_input();
                    log.push(format!(
                        "bots: phantom {} is no longer in the session: it joins again",
                        p.info.identity.gamertag
                    ));
                }
            }
        }

        // In: pending phantoms with no peer yet.
        let team_game = session::team_game(&*m, s)?;
        let playerless = b.playerless_host;
        for p in &mut b.list {
            if p.made || matches!(p.info.state, State::Failed(_)) {
                continue;
            }
            let team = match p.info.team {
                Some(t) => Some(t),
                None if team_game => Some(session::lobby_team(&*m, s)?),
                None => None,
            };
            let name = &p.info.identity.gamertag;
            let config = if playerless {
                session::playerless_config(name, team)
            } else if let Some(config) = session::config_in_ranked(&*m, s, name, team, p.rank)? {
                config
            } else {
                continue; // the local player has not joined yet
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
                    p.peer_hint = Some(stale);
                    repairs.props(session::write_peer(m, s, stale, host.as_ref(), &p.peer_identity)?);
                    repairs.identity(session::write_identity(m, s, stale, &p.peer_identity)?);
                    repairs.handicap(session::write_no_handicap(m, s, stale)?);
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
                *wrote = true;
            }
            let Some(peer) = session::add_peer(m, s, mac, &p.info.identity.gamertag)? else {
                p.info.state = State::Failed("the session has no room for another machine".into());
                continue;
            };
            *wrote = true;
            p.peer_hint = Some(peer);
            // A new peer's properties: part of its join, not a repair.
            session::write_peer(m, s, peer, host.as_ref(), &p.peer_identity)?;
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

        if in_lobby && crate::lobby::matchmaking_type(m.u32(crate::lobby::SESSION_TYPE)?) {
            session::match_roster::sync(m, s, b.match_only)?;
        }

        // Peer indexes can move when an empty peer is taken out during a join.
        // Validate each hint before using it, and resolve a name only on a move.
        if in_lobby && team_game {
            let mut peers = [0; MAX_PHANTOMS];
            let mut n = 0;
            for p in b.list.iter().filter(|p| p.made && p.info.team.is_none()) {
                if let Some(i) = session::find_peer_near(&*m, s, p.info.machine, p.peer_hint)? {
                    peers[n] = i;
                    n += 1;
                }
            }
            for (peer, from, to) in session::rebalance_teams(m, s, &peers[..n])? {
                repairs.team_moves += 1;
                let machine = session::peer_machine(&*m, s, peer)?;
                let name = b.list.iter().find(|p| p.info.machine == machine)
                    .map_or("?", |p| p.info.identity.gamertag.as_str());
                log.push(format!(
                    "bots: phantom {name} changes team {from} -> {to} to keep the teams even"
                ));
            }
        }
        if repairs.any() {
            session::bump(m, s)?;
            *wrote = true;
        }
        repairs.count(&mut b.maintenance);
        if in_lobby {
            // If a repair changed the guest, the sample above was the damaged
            // state. Never treat it as the repaired state on a later frame.
            if let Some(before) = revision_before
                && m.u32(s + session::at::UPDATE)? != before
            {
                b.lobby_membership.invalidate();
            }
        } else {
            drop(b);
            self.refresh_where(&*m, true)?;
        }
        Ok(log)
    }
}

/// The seated phantoms' records one frame rewrote, which share one
/// membership bump ([`session::bump`]).
#[derive(Default)]
struct Repairs {
    props_writes: u64,
    props_chunks: u32,
    identity_writes: u64,
    handicap_writes: u64,
    team_moves: u64,
}

impl Repairs {
    fn props(&mut self, wrote: Option<u32>) {
        if let Some(chunks) = wrote {
            self.props_writes += 1;
            self.props_chunks |= chunks;
        }
    }

    fn identity(&mut self, wrote: bool) {
        self.identity_writes += u64::from(wrote);
    }

    fn handicap(&mut self, wrote: bool) {
        self.handicap_writes += u64::from(wrote);
    }

    fn any(&self) -> bool {
        self.props_writes + self.identity_writes + self.handicap_writes + self.team_moves > 0
    }

    fn count(&self, m: &mut Maintenance) {
        m.props_writes += self.props_writes;
        m.props_diff_chunks |= self.props_chunks;
        m.identity_writes += self.identity_writes;
        m.handicap_writes += self.handicap_writes;
        m.team_moves += self.team_moves;
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
        p.set_think_mode(Mode::from_env());
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

/// Gives the joined phantom whose player is `player` an outside order
/// (docs/game/rules-world-api.md, "Bots"), or frees it (`None`).
pub fn order(player: u32, order: Option<crate::brain::Order>) -> bool {
    global().order_player(player, order)
}

mod action;

pub use action::AimView;
pub use action::SeatInput;
pub mod session;

#[cfg(test)]
mod tests;

pub mod film;
