//! The co-op probe (docs/live/live-coop.md sections 1 and 9): start a campaign
//! from a two-machine System Link lobby on the retail synchronous path,
//! and log what happens on each machine. Off unless `H2_COOP_PROBE=1`.
//!
//! What it changes, all through the feature plan:
//!
//! * **The peer-count refusal.** The session start (`0x6ec80`) reads the
//!   session's peer count (`S+0x54`) once, at `0x6ee7e`, and refuses a
//!   campaign with more than one peer at `0x6eea5`. The next call it makes
//!   is `OPTIONS_NEW` (`0x138110`), right after the checks; everything
//!   after that (the game start `0x18e8b0`, which builds the simulation
//!   world and reads the session's peers) must see the real count. So a
//!   `before` on the session start writes 1 for a campaign session with
//!   exactly two peers and at most two players, a `before` on
//!   `OPTIONS_NEW` puts the real count back, and an `after` on the session
//!   start puts it back when the start refused for another reason and never
//!   reached `OPTIONS_NEW`. Nothing else runs in that window: the start
//!   makes no other call between its entry and `OPTIONS_NEW`. The player
//!   check (`0x6ee9b`, more than two players) is left as it is: two
//!   machines with one player each pass it. A co-op over Live host alone
//!   (one peer, one player) is shown two players in the same window, so
//!   the options' co-op byte is 1 for the friends who join in progress
//!   ([`start_bypass`]).
//! * **The campaign lobby.** On the host, once a second machine is in the
//!   System Link lobby, the probe calls the lobby's own setters at the
//!   start of a game tick (the menu ticks too): `LOBBY_SET_MAP`
//!   (`0x19a6f2`, campaign 1 and the level's map id) and
//!   `LOBBY_SET_DIFFICULTY` (`0x19a76d`), which also clears the variant so
//!   the session picks the synchronous protocol. Both replicate to the
//!   guest as session parameters. The tick then runs as usual.
//! * **The peer limit.** The host's game-start parameters (`0x6e720`) set
//!   a campaign session's peer limit (`S+0x4990`) to 1; the probe sets it
//!   back to 2 after it returns.
//! * **Joinable.** The Live presence predicate (`0x63e00`) answers false
//!   for any campaign session; the probe answers as retail would without
//!   that clause, while fewer than two players are in. (System Link joins
//!   are gated by the peer and player limits, not by this predicate.)
//!
//! What it logs, every line prefixed `coop-probe:`: the session (state,
//! peers, players, host, protocol, difficulty, map) whenever it changes;
//! the simulation mode and the retail out-of-sync and ended latches when
//! they change; ticks that come more than half a second apart (a peer
//! waiting for updates); checkpoints, reverts, BSP switches and
//! cinematics; and every tick of a game, one line per game-state pool, to
//! `<data>/coop-probe/<time>-<pid>-<role>.hashes` for `h2 coop-probe diff`.
//!
//! The decisions are pure functions over [`Gm`], tested without a guest.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use guest_layout::points::{self, Point};
use hooks::{Call, Flow};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use crate::layout_glue::Gm;
use crate::level_boot::{self, Difficulty};

/// The feature id (`H2_FEATURES=-coop-probe` turns it off again).
pub const ID: &str = "coop-probe";

// ---- configuration ----

/// The level the probe asks for when `H2_COOP_PROBE_MAP` is unset.
pub const DEFAULT_MAP: &str = "01a_tutorial";

/// What `H2_COOP_PROBE*` asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The campaign level the host sets, a map file stem.
    pub map: String,
    pub difficulty: Difficulty,
}

/// Reads `H2_COOP_PROBE`, `H2_COOP_PROBE_MAP` and
/// `H2_COOP_PROBE_DIFFICULTY` through `get`. `None`: the probe is off
/// (unset, empty, `0`, `off`, `false`, `no`). Anything other than those
/// and `1`, `on`, `true`, `yes` is an error, as is a bad difficulty.
pub fn parse(get: impl Fn(&str) -> Option<String>) -> Result<Option<Config>, String> {
    let on = get("H2_COOP_PROBE").unwrap_or_default();
    match on.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "off" | "false" | "no" => return Ok(None),
        "1" | "on" | "true" | "yes" => {}
        v => return Err(format!("H2_COOP_PROBE={v:?}: use 1 or 0")),
    }
    let map = get("H2_COOP_PROBE_MAP")
        .map(|m| m.trim().trim_end_matches(".map").to_owned())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_MAP.to_owned());
    let difficulty = match get("H2_COOP_PROBE_DIFFICULTY").map(|d| d.trim().to_owned()) {
        Some(d) if !d.is_empty() => {
            Difficulty::parse(&d).map_err(|e| format!("H2_COOP_PROBE_DIFFICULTY: {e}"))?
        }
        _ => Difficulty::Normal,
    };
    Ok(Some(Config { map, difficulty }))
}

/// The level the host sets, resolved against the disc.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Want {
    pub name: String,
    pub campaign: u32,
    pub map: u32,
    pub difficulty: u16,
}

impl Want {
    /// From a level boot request; only a campaign level with ids will do.
    pub fn from_request(r: &level_boot::Request) -> Result<Want, String> {
        if r.kind != level_boot::Kind::Campaign {
            return Err(format!("{} is not a campaign level", r.name));
        }
        if r.campaign == u32::MAX || r.map == u32::MAX {
            return Err(format!("the globals' level list does not name {}", r.name));
        }
        Ok(Want {
            name: r.name.clone(),
            campaign: r.campaign,
            map: r.map,
            difficulty: r.difficulty.index(),
        })
    }
}

// ---- the session (docs/live/live-coop.md 1.1, 1.4) ----

/// Squad session fields. `S+0x54` is the count of the 16-entry peer array
/// at `+0x58` (stride `0x10c`; `0x6e798` builds a peer mask from it).
pub mod layout {
    pub const LOCAL_PEER: u32 = 0x40;
    /// The local peer the parameter setters compare (`0x63f00`).
    pub const LOCAL_PEER_PARAMS: u32 = 0x50;
    pub const PEERS: u32 = 0x54;
    pub const PLAYERS: u32 = 0x1118;
    pub const PARAMS_VERSION: u32 = 0x4978;
    /// The squad's stage (set only by `0x5a220`): 1 in a pregame lobby,
    /// the only stage the lobby setters accept (`0x63f00`). Not the
    /// network class (`crate::variant_ext::SESSION_NETWORK_CLASS`).
    pub const CLASS: u32 = 0x4980;
    pub const PRIVACY: u32 = 0x498c;
    pub const MAX_PEERS: u32 = 0x4990;
    pub const MAX_PLAYERS: u32 = 0x4994;
    pub const CAMPAIGN: u32 = 0x4d08;
    pub const MAP: u32 = 0x4d0c;
    pub const PROTOCOL: u32 = 0x4dac;
    /// Word; `0xffff` is "not campaign".
    pub const DIFFICULTY: u32 = 0x5dd0;
    pub const HOST_PEER: u32 = 0x72d8;
    pub const STATE: u32 = 0x741c;
}

/// The network life cycle's state; the joinable predicate refuses 6 to 9.
const LIFE_CYCLE: u32 = 0x0052_7334;
pub const NO_DIFFICULTY: u16 = 0xffff;
/// Two machines, two players: where the probe stops, and co-op over Live
/// by default (`coop.max_players`, [`Arm::set_max_players`]).
pub const MAX_PEERS: u32 = 2;
pub const MAX_PLAYERS: u32 = 2;
/// The most players a session, its options and the synchronous update
/// record carry: 16 peer entries at `S+0x58`, 16 player entries at
/// `S+0x4f90` (`0x6ee12` loops 16), 16 options players (`+0x2d4`, `0xe4`
/// each) and 16 update entries at `+0x610` (docs/live/live-coop.md 1.3, 12).
pub const CEILING: u32 = 16;
/// The player check the session start makes (`0x6ee9b`: more than two
/// players refuses a campaign start).
pub const START_PLAYER_CHECK: u32 = 2;

/// `n` held to what co-op supports: 2 to [`CEILING`].
pub fn clamp_players(n: i64) -> u32 {
    n.clamp(i64::from(MAX_PLAYERS), i64::from(CEILING)) as u32
}

/// A squad session, read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub addr: u32,
    pub state: u32,
    pub peers: u32,
    pub players: u32,
    pub local_peer: u32,
    pub local_peer_params: u32,
    pub host_peer: u32,
    pub class: u32,
    pub privacy: u32,
    pub max_peers: u32,
    pub max_players: u32,
    pub campaign: u32,
    pub map: u32,
    pub protocol: u32,
    pub difficulty: u16,
}

impl Session {
    /// The states in which the session's fields mean anything (every
    /// reader checks 3 to 8).
    pub fn live(&self) -> bool {
        (3..=8).contains(&self.state)
    }
    pub fn is_campaign(&self) -> bool {
        self.difficulty != NO_DIFFICULTY
    }
    /// Host as the session start sees it (`0x6ec80`).
    pub fn is_host(&self) -> bool {
        self.host_peer == self.local_peer
    }
    /// Host as the parameter setters see it (`0x63f00`).
    pub fn can_set_params(&self) -> bool {
        self.host_peer == self.local_peer_params && self.class == 1
    }
    /// One line for the log.
    pub fn describe(&self) -> String {
        let difficulty = if self.is_campaign() {
            self.difficulty.to_string()
        } else {
            "none".into()
        };
        format!(
            "session {:#x}: state {}, peers {} (max {}), players {} (max {}), {} (host peer {}, local {}/{}), \
             protocol {} ({}), difficulty {difficulty}, campaign {} map {}, privacy {}, class {}",
            self.addr,
            self.state,
            self.peers,
            self.max_peers as i32,
            self.players,
            self.max_players as i32,
            if self.is_host() { "host" } else { "guest" },
            self.host_peer as i32,
            self.local_peer as i32,
            self.local_peer_params as i32,
            self.protocol as i32,
            protocol_name(self.protocol),
            self.campaign as i32,
            self.map as i32,
            self.privacy,
            self.class,
        )
    }
}

pub fn protocol_name(p: u32) -> &'static str {
    match p {
        0 => "local",
        1 => "synchronous",
        2 => "distributed",
        _ => "?",
    }
}

/// The session at `addr`.
pub fn read_session_at<M: Gm + ?Sized>(m: &M, addr: u32) -> Option<Session> {
    use layout::*;
    let f = |o: u32| m.u32(addr.wrapping_add(o));
    Some(Session {
        addr,
        state: f(STATE)?,
        peers: f(PEERS)?,
        players: f(PLAYERS)?,
        local_peer: f(LOCAL_PEER)?,
        local_peer_params: f(LOCAL_PEER_PARAMS)?,
        host_peer: f(HOST_PEER)?,
        class: f(CLASS)?,
        privacy: f(PRIVACY)?,
        max_peers: f(MAX_PEERS)?,
        max_players: f(MAX_PLAYERS)?,
        campaign: f(CAMPAIGN)?,
        map: f(MAP)?,
        protocol: f(PROTOCOL)?,
        difficulty: m.u16(addr.wrapping_add(DIFFICULTY))?,
    })
}

/// The squad session, when the network life cycle has one (the gates of
/// `0x63e00` and the setters: ready byte set, a session, a nonzero state).
pub fn read_session<M: Gm + ?Sized>(m: &M) -> Option<Session> {
    if m.u8(points::NETWORK_READY.base)? == 0 {
        return None;
    }
    let addr = m.u32(points::SQUAD_SESSION.base)?;
    if addr == 0 {
        return None;
    }
    let s = read_session_at(m, addr)?;
    (s.state != 0).then_some(s)
}

// ---- decisions ----

/// What the session start's `before` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bypass {
    /// Leave the start as retail; why.
    Retail(&'static str),
    /// Show the start one peer (and at most two players) until
    /// `OPTIONS_NEW`; `real` peers and, when it was changed, `players` go
    /// back.
    Fake { real: u32, players: Option<u32> },
    /// Past the limit; retail refuses it.
    Refuse(String),
}

/// The peer-count decision for a session start, for a co-op limit of `max`
/// players (one per console).
///
/// The start reads the player count once (`0x6ee69`, into EDI) and uses it
/// twice: the refusal at `0x6ee9b` (more than two) and, after
/// `OPTIONS_NEW`, the options' "more than one player" byte (`0x6ef61`,
/// `setg` on `EDI > 1`). So for three or more players the window also
/// shows two: the refusal passes and the byte is 1, as it is for any
/// count above one. Everything after `OPTIONS_NEW` reads the real count.
pub fn peer_bypass(s: &Session, max: u32) -> Bypass {
    if !s.live() {
        return Bypass::Retail("the session is not in a lobby or game state");
    }
    if !s.is_campaign() {
        return Bypass::Retail("not a campaign session");
    }
    if s.peers <= 1 {
        return Bypass::Retail("one peer: retail starts it");
    }
    if s.peers > max {
        return Bypass::Refuse(format!("{} peers: co-op stops at {max}", s.peers));
    }
    if s.players > max {
        return Bypass::Refuse(format!("{} players: co-op stops at {max}", s.players));
    }
    Bypass::Fake {
        real: s.peers,
        players: (s.players > START_PLAYER_CHECK).then_some(s.players),
    }
}

/// The session start's decision with the solo start (`solo`: co-op over
/// Live with `coop.join_in_progress` on, [`Arm::solo_start`]): one console
/// alone with one player in a networked campaign session is shown two
/// players until `OPTIONS_NEW`, so the options' "more than one player"
/// byte (`0x6ef61`) is 1 and the game is co-op (respawn, the co-op loss
/// rule) for the friends who join it in progress
/// (docs/live/live-coop-join.md 13.4). Anything else: [`peer_bypass`].
pub fn start_bypass(s: &Session, max: u32, solo: bool) -> Bypass {
    if solo
        && s.live()
        && s.is_campaign()
        && s.class != 0
        && s.peers == 1
        && s.players == 1
    {
        return Bypass::Fake {
            real: 1,
            players: Some(1),
        };
    }
    peer_bypass(s, max)
}

/// The session start's line for a [`Bypass::Fake`]: one console alone, or
/// `real` peers shown as one.
pub fn fake_line(real: u32) -> String {
    if real == 1 {
        format!(
            "session start: campaign with 1 peer (alone): showing the start {START_PLAYER_CHECK} players until OPTIONS_NEW, so the game is co-op; friends join in progress"
        )
    } else {
        format!(
            "session start: campaign with {real} peers: showing the start 1 peer until OPTIONS_NEW"
        )
    }
}

/// What `0x63e00` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoinInputs {
    /// The ready byte is set and there is a session.
    pub ready: bool,
    pub state: u32,
    pub privacy: u32,
    pub players: u32,
    pub max_players: u32,
    pub life_cycle: u32,
    pub difficulty: u16,
}

/// The retail predicate (`limit` `None`), or the probe's: the same without
/// the campaign clause, while fewer than `limit` players are in.
pub fn joinable(j: &JoinInputs, limit: Option<u32>) -> bool {
    if !j.ready || !(3..=8).contains(&j.state) || j.privacy != 0 {
        return false;
    }
    if j.players >= j.max_players || (6..=9).contains(&j.life_cycle) {
        return false;
    }
    if j.difficulty == NO_DIFFICULTY {
        return true;
    }
    limit.is_some_and(|max| j.players < max)
}

pub fn read_join<M: Gm + ?Sized>(m: &M) -> JoinInputs {
    let Some(s) = read_session(m) else {
        return JoinInputs::default();
    };
    JoinInputs {
        ready: true,
        state: s.state,
        privacy: s.privacy,
        players: s.players,
        max_players: s.max_players,
        life_cycle: m.u32(LIFE_CYCLE).unwrap_or(0),
        difficulty: s.difficulty,
    }
}

/// Where the host's lobby setup stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// Nothing to do yet (or ever, on a guest); why.
    Wait(String),
    /// The session already holds the level and difficulty.
    Done,
    /// Call the setters now.
    Due,
}

pub fn host_setup(s: &Session, want: &Want, max: u32) -> Setup {
    if !s.live() {
        return Setup::Wait(format!("session state {}", s.state));
    }
    if !s.can_set_params() {
        return Setup::Wait("not the session's host".into());
    }
    if s.campaign == want.campaign && s.map == want.map && s.difficulty == want.difficulty {
        return Setup::Done;
    }
    if s.peers < MAX_PEERS {
        return Setup::Wait(format!(
            "waiting for a second machine ({} of {MAX_PEERS})",
            s.peers
        ));
    }
    if s.peers > max {
        return Setup::Wait(format!("{} machines: co-op stops at {max}", s.peers));
    }
    Setup::Due
}

/// The limits to write after the host's game-start parameters (`0x6e720`
/// sets a campaign's peer limit to 1 at `0x6e8eb` and its player limit to
/// 2 at `0x6e8aa`), for a co-op limit of `max`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LimitsFix {
    pub peers: Option<u32>,
    pub players: Option<u32>,
}

pub fn limits_fix(s: &Session, max: u32) -> LimitsFix {
    if !(s.live() && s.is_campaign()) {
        return LimitsFix::default();
    }
    LimitsFix {
        peers: (s.max_peers < max).then_some(max),
        players: (s.max_players < max).then_some(max),
    }
}

/// One guest call of the lobby setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupCall {
    pub point: &'static Point,
    pub eax: Option<u32>,
    pub esi: Option<u32>,
    /// Stack arguments, the first at `[esp+4]`.
    pub args: [Option<u32>; 1],
}

/// The calls that put `want` on the session, in order (as the campaign
/// menu's `0x199af6` then `0x199b00` make them).
pub fn setup_calls(want: &Want) -> [SetupCall; 2] {
    [
        SetupCall {
            point: &points::LOBBY_SET_MAP,
            eax: None,
            esi: Some(want.map),
            args: [Some(want.campaign)],
        },
        SetupCall {
            point: &points::LOBBY_SET_DIFFICULTY,
            eax: Some(u32::from(want.difficulty)),
            esi: None,
            args: [None],
        },
    ]
}

// ---- the simulation (docs/live/live-coop.md 1.1, 1.3) ----

/// The simulation world's mode word (`SIM_GLOBALS + SIM_MODE`): 0 none,
/// 1 local, 2 the synchronous server, 3 a synchronous client
/// (`mode_name`). The Forge co-op link reads it to say which end of the
/// link this machine is.
pub const SIM_MODE: u32 = 0x8;
const SIM_UPDATE: u32 = 0x28;
const SIM_DESYNC: u32 = 0x2c;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sim {
    pub mode: u32,
    pub update: u32,
    pub desync: u8,
    pub ended: u8,
}

pub fn read_sim<M: Gm + ?Sized>(m: &M) -> Option<Sim> {
    let g = m.u32(points::SIM_GLOBALS.base)?;
    if g == 0 {
        return None;
    }
    Some(Sim {
        mode: m.u32(g.wrapping_add(SIM_MODE))?,
        update: m.u32(g.wrapping_add(SIM_UPDATE))?,
        desync: m.u8(g.wrapping_add(SIM_DESYNC))?,
        ended: m.u8(points::SIM_ENDED.base).unwrap_or(0),
    })
}

/// The simulation world's mode name (`[0x4cf77c]+8`). The world numbers its
/// modes otherwise than the game (options `+4`, the table at `0x468c64`):
/// `0x68c00` maps game 1..5 to world 1, 3, 2, 5, 4 through the jump table at
/// `0x68da8`, so here 2 is the synchronous server and 4 the distributed one
/// (`bots::phantom::session::world_mode` is that map).
pub fn mode_name(mode: u32) -> &'static str {
    match mode {
        0 => "none",
        1 => "local",
        2 => "sync-server",
        3 => "sync-client",
        4 => "dist-server",
        5 => "dist-client",
        _ => "?",
    }
}

/// The hash log's role word for a simulation world mode (the servers, 2 and
/// 4, are the host; the clients, 3 and 5, the guests).
pub fn role(mode: u32) -> &'static str {
    match mode {
        2 | 4 => "host",
        3 | 5 => "guest",
        _ => "local",
    }
}

/// What changed between two simulation readings, as log lines.
pub fn sim_changes(before: Option<Sim>, now: Sim, tick: Option<u32>) -> Vec<String> {
    let at = tick.map_or_else(String::new, |t| format!(" at tick {t}"));
    let mut out = Vec::new();
    let b = before.unwrap_or(Sim {
        mode: u32::MAX,
        ..Sim::default()
    });
    if b.mode != now.mode {
        out.push(format!(
            "simulation mode {} ({}){at}",
            mode_name(now.mode),
            now.mode
        ));
    }
    if now.desync != 0 && b.desync == 0 {
        out.push(format!(
            "OUT OF SYNC: the retail latch (simulation globals +0x2c) is set{at}, update {}",
            now.update
        ));
    }
    if now.desync == 0 && b.desync != 0 && before.is_some() {
        out.push(format!("the out-of-sync latch cleared{at}"));
    }
    if now.ended != 0 && b.ended == 0 {
        out.push(format!(
            "the simulation's ended latch (0x4cf772) is set{at}"
        ));
    }
    out
}

/// Ticks further apart than this are a stall.
pub const STALL_MS: u128 = 500;

// ---- the hash log ----

/// The lines of one tick: the whole block, then each pool.
pub fn tick_lines(
    tick: u32,
    whole: u64,
    pools: &[lockstep::pools::Pool],
    hashes: &[u64],
) -> String {
    use lockstep::pools::{Line, WHOLE, log_name};
    let mut s = Line {
        tick,
        pool: WHOLE.into(),
        hash: whole,
    }
    .to_string();
    s.push('\n');
    for (p, h) in pools.iter().zip(hashes) {
        s += &Line {
            tick,
            pool: log_name(&p.name),
            hash: *h,
        }
        .to_string();
        s.push('\n');
    }
    s
}

/// The game-state block: `0x3fe000` bytes behind [`points::GAME_STATE_BASE`].
const BLOCK_LEN: u32 = 0x003f_e000;
/// Flush the log this often (ticks), so a crash loses little.
const FLUSH_EVERY: u32 = 300;

struct HashLog {
    out: BufWriter<File>,
    path: PathBuf,
    pools: Vec<lockstep::pools::Pool>,
    base: u32,
    buf: Vec<u8>,
    ticks: u32,
}

impl HashLog {
    fn open(dir: &Path, role: &str, header: &[String]) -> std::io::Result<HashLog> {
        std::fs::create_dir_all(dir)?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = dir.join(format!("{secs}-{}-{role}.hashes", std::process::id()));
        let mut out = BufWriter::new(File::create(&path)?);
        writeln!(out, "{}", lockstep::pools::LOG_HEADER)?;
        for h in header {
            writeln!(out, "# {h}")?;
        }
        Ok(HashLog {
            out,
            path,
            pools: Vec::new(),
            base: 0,
            buf: vec![0; BLOCK_LEN as usize],
            ticks: 0,
        })
    }

    fn note(&mut self, s: &str) {
        let _ = writeln!(self.out, "# {s}");
    }

    /// Hashes the block and writes the tick. Re-cuts the pools when the
    /// block moved (or on the first tick).
    fn tick(&mut self, g: &runtime::Guest, tick: u32) -> Result<(), String> {
        let base = g
            .u32(points::GAME_STATE_BASE.base)
            .map_err(|e| format!("{e:?}"))?;
        if base == 0 {
            return Err("no game-state block yet".into());
        }
        g.read_bytes(base, &mut self.buf, 1)
            .map_err(|e| format!("reading the block: {e:?}"))?;
        if base != self.base || self.pools.is_empty() {
            self.base = base;
            self.pools = lockstep::pools::find_pools(&self.buf);
            let n = self.pools.len();
            self.note(&format!("tick {tick} pools {n} at {base:#x}"));
            eprintln!(
                "coop-probe: hashing the game-state block at {base:#x}: {n} pools; writing {}",
                self.path.display()
            );
        }
        let hashes = lockstep::pools::hash_pools(&self.buf, &self.pools);
        let whole = lockstep::pools::hash_block(&self.buf);
        self.out
            .write_all(tick_lines(tick, whole, &self.pools, &hashes).as_bytes())
            .map_err(|e| e.to_string())?;
        self.ticks += 1;
        if self.ticks.is_multiple_of(FLUSH_EVERY) {
            let _ = self.out.flush();
        }
        Ok(())
    }
}

// ---- the feature ----

/// What the session start's window changed: the session, its real peer
/// count and, when it was shown two, its real player count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    addr: u32,
    peers: u32,
    players: Option<u32>,
}

#[derive(Default)]
struct State {
    /// The counts the session start must get back.
    pending: Option<Pending>,
    last_session: Option<String>,
    last_sim: Option<Sim>,
    last_join: Option<bool>,
    setup_wait: Option<String>,
    setup_attempts: u32,
    setup_last_tick: Option<u32>,
    setup_done: bool,
    /// The level the setup state above is for.
    setup_for: Option<Want>,
    log: Option<HashLog>,
    log_error: bool,
    last_tick_at: Option<Instant>,
    last_tick: Option<u32>,
    /// "game end" was said since the last game start. The game-end path
    /// (`GAME_END`) can return on every tick, latch set or not (a log had
    /// the line once a tick, thousands of times); it is said once.
    end_said: bool,
}

impl State {
    /// A game started: its end is said again.
    fn started(&mut self) {
        self.end_said = false;
    }
    /// The game-end path returned: whether to say so (the first time
    /// since the last start).
    fn end_first(&mut self) -> bool {
        !std::mem::replace(&mut self.end_said, true)
    }
}

/// The feature id of the co-op over Live hooks (`H2_FEATURES=-coop-live`
/// turns them off): the probe's, armed only for a co-op session.
pub const LIVE_ID: &str = "coop-live";

/// When the hooks act. The probe (`H2_COOP_PROBE`) is armed from boot, with
/// the level the host sets; co-op over Live (`crate::coop_live`) arms them
/// only while this console hosts or joined a co-op session, the host with
/// its chosen level. Disarmed, every hook lets the game run as retail and
/// nothing is logged.
#[derive(Debug)]
pub struct Arm {
    forced: bool,
    on: AtomicBool,
    want: Mutex<Option<Want>>,
    /// The host's co-op player limit (`coop.max_players`), 2 to
    /// [`CEILING`].
    max_players: AtomicU32,
}

impl Default for Arm {
    fn default() -> Arm {
        Arm {
            forced: false,
            on: AtomicBool::new(false),
            want: Mutex::new(None),
            max_players: AtomicU32::new(MAX_PLAYERS),
        }
    }
}

impl Arm {
    /// Armed for the whole run (the probe).
    pub fn forced(want: Option<Want>) -> Arc<Arm> {
        Arc::new(Arm {
            forced: true,
            on: AtomicBool::new(true),
            want: Mutex::new(want),
            ..Arm::default()
        })
    }
    /// Off until a co-op session arms it.
    pub fn session() -> Arc<Arm> {
        Arc::new(Arm::default())
    }
    pub fn is_forced(&self) -> bool {
        self.forced
    }
    pub fn active(&self) -> bool {
        self.forced || self.on.load(Ordering::SeqCst)
    }
    /// Arms the hooks: the host with the level it sets on the lobby, a
    /// guest with none. A forced arm keeps its own level.
    pub fn arm(&self, want: Option<Want>) {
        if !self.forced {
            *self.want.lock().unwrap_or_else(|e| e.into_inner()) = want;
        }
        self.on.store(true, Ordering::SeqCst);
    }
    /// Back to retail (a forced arm stays on).
    pub fn disarm(&self) {
        if !self.forced {
            *self.want.lock().unwrap_or_else(|e| e.into_inner()) = None;
            self.on.store(false, Ordering::SeqCst);
        }
    }
    /// Sets the host's player limit (`coop.max_players`), held to 2 to
    /// [`CEILING`]; the value it took.
    pub fn set_max_players(&self, n: i64) -> u32 {
        let n = clamp_players(n);
        self.max_players.store(n, Ordering::SeqCst);
        n
    }
    /// The host's player limit.
    pub fn max_players(&self) -> u32 {
        self.max_players.load(Ordering::SeqCst)
    }
    /// Whether a co-op host may start alone, friends joining in progress
    /// (docs/live/live-coop-join.md 13.4): co-op over Live (not the
    /// probe's forced arm) with `coop.join_in_progress` on.
    pub fn solo_start(&self) -> bool {
        !self.forced && crate::coop_jip::enabled()
    }
    /// The limit a session's checks use on this console: the host's own
    /// setting; on a guest, [`CEILING`], since the host's lobby already
    /// held the start to its limit and a guest that refused it would only
    /// break the game the host started.
    pub fn limit_for(&self, s: &Session) -> u32 {
        if s.is_host() {
            self.max_players()
        } else {
            CEILING
        }
    }
    /// Why a machine snapshot must wait (docs/runtime/snapshots.md): armed,
    /// the hooks follow a co-op session whose peers and Live state no
    /// snapshot holds. The probe's arm is on for the whole run.
    pub fn snapshot_live(&self) -> Option<String> {
        if self.forced {
            return Some("the co-op probe is on".into());
        }
        self.on
            .load(Ordering::SeqCst)
            .then(|| "a co-op session has armed the co-op hooks".into())
    }
    /// The level the host sets, when armed as host.
    pub fn want(&self) -> Option<Want> {
        if !self.active() {
            return None;
        }
        self.want.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Why the probe (or the co-op over Live hooks) can't be snapshotted now:
/// armed ([`Arm::snapshot_live`]), a session start's peer or player count
/// is shown changed until `OPTIONS_NEW` puts it back, or a hash log is
/// open (a file the snapshot does not hold). Disarmed, what is left is the
/// last values said and the host setup's retry count, which only shape the
/// log and the next session's first setup attempt.
fn probe_live(arm: &Arm, st: &Mutex<State>) -> Option<String> {
    if let Some(why) = arm.snapshot_live() {
        return Some(why);
    }
    let s = lock(st);
    if s.pending.is_some() {
        return Some("a session start's peer count is shown changed".into());
    }
    s.log
        .as_ref()
        .map(|log| format!("the co-op hash log {} is open", log.path.display()))
}

/// The probe as a feature: added when `H2_COOP_PROBE` is on (armed from
/// boot) or when Live is on (armed by a co-op session).
pub struct Probe {
    id: &'static str,
    arm: Arc<Arm>,
    dir: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Probe {
    /// The probe, armed from boot with the level the host sets.
    pub fn new(want: Option<Want>, dir: PathBuf) -> Probe {
        Probe {
            id: ID,
            arm: Arm::forced(want),
            dir,
            state: Arc::default(),
        }
    }
    /// The co-op over Live hooks, armed by `arm`.
    pub fn armed(arm: Arc<Arm>, dir: PathBuf) -> Probe {
        let id = if arm.is_forced() { ID } else { LIVE_ID };
        Probe {
            id,
            arm,
            dir,
            state: Arc::default(),
        }
    }
}

/// Every point the probe hooks.
pub static HOOKED: &[&Point] = &[
    &points::SESSION_GAME_START,
    &points::OPTIONS_NEW,
    &points::SESSION_JOINABLE,
    &points::SESSION_GAME_LIMITS,
    &points::GAME_TICK,
    &points::GAME_ACTIVATE,
    &points::GAME_END,
    &points::GAME_STATE_SAVE,
    &points::GAME_STATE_REVERT,
    &points::BSP_SWITCH,
    &points::CINEMATIC_START,
    &points::CINEMATIC_STOP,
];

fn say(s: &str) {
    eprintln!("coop-probe: {s}");
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn game_tick<M: Gm + ?Sized>(m: &M) -> Option<u32> {
    let p = m.u32(points::GAME_TIME.base)?;
    m.u32(p.wrapping_add(8))
}

/// Logs an event, in the hash log too when one is open.
fn event(state: &Mutex<State>, g: &runtime::Guest, what: &str) {
    let tick = game_tick(g);
    let at = tick.map_or_else(|| "before the first tick".into(), |t| format!("tick {t}"));
    say(&format!("{what} at {at}"));
    if let (Some(t), Some(log)) = (tick, lock(state).log.as_mut()) {
        log.note(&format!("tick {t} {what}"));
        let _ = log.out.flush();
    }
}

impl hooks::Feature for Probe {
    fn manifest(&self) -> hooks::Manifest {
        // On when added: it is only added when H2_COOP_PROBE is on, and
        // H2_FEATURES=-coop-probe still turns it off.
        let title = if self.id == ID {
            "Co-op probe: the retail synchronous campaign path"
        } else {
            "Co-op over Live: the retail synchronous campaign path, armed for co-op sessions"
        };
        hooks::Manifest::new(self.id, title).doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The peer-count refusal: show one peer from the start's entry to
        // OPTIONS_NEW.
        let st = Arc::clone(&self.state);
        let arm = Arc::clone(&self.arm);
        b.raw().before(&points::SESSION_GAME_START, "one peer for the campaign check", move |c| {
            if !arm.active() {
                return Ok(Flow::Continue);
            }
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            let addr = c.cpu.edx;
            let Some(s) = read_session_at(&*mem, addr) else {
                say(&format!("session start: cannot read the session at {addr:#x}"));
                return Ok(Flow::Continue);
            };
            say(&format!("session start: {}", s.describe()));
            match start_bypass(&s, arm.limit_for(&s), arm.solo_start()) {
                Bypass::Retail(why) => say(&format!("session start: retail ({why})")),
                Bypass::Refuse(why) => say(&format!("session start: refused ({why}); retail refuses too")),
                Bypass::Fake { real, players } => {
                    let peers_ok = Gm::set_u32(&mut *mem, addr.wrapping_add(layout::PEERS), 1);
                    let players_ok = players.is_none_or(|_| {
                        Gm::set_u32(&mut *mem, addr.wrapping_add(layout::PLAYERS), START_PLAYER_CHECK)
                    });
                    if peers_ok && players_ok {
                        lock(&st).pending = Some(Pending { addr, peers: real, players });
                        say(&fake_line(real));
                        if let Some(n) = players.filter(|_| real > 1) {
                            say(&format!(
                                "session start: {n} players: showing the start {START_PLAYER_CHECK} until OPTIONS_NEW"
                            ));
                        }
                    } else {
                        // Put back whatever was written.
                        let _ = Gm::set_u32(&mut *mem, addr.wrapping_add(layout::PEERS), real);
                        say("session start: could not write the peer or player count");
                    }
                }
            }
            Ok(Flow::Continue)
        });
        let st = Arc::clone(&self.state);
        b.raw()
            .before(&points::OPTIONS_NEW, "restore the peer count", move |c| {
                // Not gated on the arm: a pending count always goes back.
                restore(&st, c, "before OPTIONS_NEW");
                Ok(Flow::Continue)
            });
        let st = Arc::clone(&self.state);
        let arm2 = Arc::clone(&self.arm);
        b.raw().after(
            &points::SESSION_GAME_START,
            "restore the peer count, report",
            move |c| {
                let pending = lock(&st).pending.is_some();
                restore(&st, c, "after a refused start");
                if !pending && !arm2.active() {
                    return Ok(());
                }
                let started = c.cpu.eax & 0xff != 0;
                say(&format!(
                    "session start returned {}",
                    if started {
                        "1 (the game starts)"
                    } else {
                        "0 (refused)"
                    }
                ));
                Ok(())
            },
        );

        // Joinable while a campaign session has room.
        let st = Arc::clone(&self.state);
        let arm = Arc::clone(&self.arm);
        b.raw().before(&points::SESSION_JOINABLE, "joinable campaign", move |c| {
            if !arm.active() {
                return Ok(Flow::Continue);
            }
            let rt = Arc::clone(c.ctx.runtime());
            let j = read_join(&*rt.memory());
            let limit = read_session(&*rt.memory()).map_or(MAX_PLAYERS, |s| arm.limit_for(&s));
            let (retail, probe) = (joinable(&j, None), joinable(&j, Some(limit)));
            {
                let mut s = lock(&st);
                if s.last_join != Some(probe) {
                    s.last_join = Some(probe);
                    say(&format!(
                        "joinable: {probe} (retail {retail}; players {} of {}, difficulty {:#x})",
                        j.players, j.max_players, j.difficulty
                    ));
                }
            }
            if probe && !retail {
                c.cpu.eax = (c.cpu.eax & !0xff) | 1;
                return Ok(Flow::Return(c.ret(0)?));
            }
            Ok(Flow::Continue)
        });

        // The peer limit the host's game start sets for a campaign.
        let arm = Arc::clone(&self.arm);
        b.raw()
            .after(&points::SESSION_GAME_LIMITS, "peer limit 2", move |c| {
                if !arm.active() {
                    return Ok(());
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let Some(s) = read_session_at(&*mem, c.cpu.ebx) else {
                    return Ok(());
                };
                let fix = limits_fix(&s, arm.limit_for(&s));
                if let Some(n) = fix.peers
                    && Gm::set_u32(&mut *mem, s.addr.wrapping_add(layout::MAX_PEERS), n)
                {
                    say(&format!(
                        "game limits: campaign peer limit {} -> {n}",
                        s.max_peers as i32
                    ));
                }
                if let Some(n) = fix.players
                    && Gm::set_u32(&mut *mem, s.addr.wrapping_add(layout::MAX_PLAYERS), n)
                {
                    say(&format!(
                        "game limits: campaign player limit {} -> {n}",
                        s.max_players as i32
                    ));
                }
                Ok(())
            });

        // The host's lobby setup, at the start of a tick.
        // A forced arm with no level never sets one; a session arm may be
        // given one at any time.
        if !self.arm.is_forced() || self.arm.want().is_some() {
            let st = Arc::clone(&self.state);
            let arm = Arc::clone(&self.arm);
            b.raw()
                .before(&points::GAME_TICK, "host lobby setup", move |c| {
                    match arm.want() {
                        Some(want) => lobby_setup(&st, &want, arm.max_players(), c),
                        None => Ok(Flow::Continue),
                    }
                })
                // Our skulls' calls re-enter the tick; the setup runs
                // inside them.
                .after_feature(crate::skulls_custom::FEATURE);
        }

        // Every tick: session, simulation, stalls, hashes.
        let st = Arc::clone(&self.state);
        let arm = Arc::clone(&self.arm);
        b.frame().on_tick("session, mode and hashes", move |o| {
            if !arm.active() {
                return;
            }
            let g = o.guest();
            on_tick(&st, &g);
        });

        let st = Arc::clone(&self.state);
        let dir = self.dir.clone();
        let arm = Arc::clone(&self.arm);
        b.game().on_start("open the hash log", move |o| {
            lock(&st).started();
            if !arm.active() {
                return;
            }
            let g = o.guest();
            on_start(&st, &g, &dir);
        });
        let st = Arc::clone(&self.state);
        let arm = Arc::clone(&self.arm);
        b.game().on_end("close the hash log", move |o| {
            let g = o.guest();
            let say_it = {
                let mut s = lock(&st);
                (arm.active() || s.log.is_some()) && s.end_first()
            };
            if say_it {
                event(&st, &g, "game end");
            }
            if let Some(mut log) = lock(&st).log.take() {
                let _ = log.out.flush();
                say(&format!(
                    "wrote {} ticks to {}",
                    log.ticks,
                    log.path.display()
                ));
            }
        });

        for (point, what) in [
            (&points::GAME_STATE_SAVE, "checkpoint save"),
            (&points::GAME_STATE_REVERT, "checkpoint revert"),
            (&points::BSP_SWITCH, "bsp switch"),
            (&points::CINEMATIC_START, "cinematic start"),
            (&points::CINEMATIC_STOP, "cinematic stop"),
        ] {
            let st = Arc::clone(&self.state);
            let arm = Arc::clone(&self.arm);
            b.raw()
                .observe_before(point, what, move |o| {
                    if arm.active() {
                        event(&st, &o.guest(), what)
                    }
                })
                .optional();
        }
        let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || probe_live(&arm, &st)));
        Ok(())
    }
}

fn restore(st: &Mutex<State>, c: &mut Call, when: &str) {
    let Some(p) = lock(st).pending.take() else {
        return;
    };
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    for line in restore_counts(&mut *mem, p, when) {
        say(&line);
    }
}

/// Writes back what the session start's window changed; the log lines.
fn restore_counts<M: Gm + ?Sized>(m: &mut M, p: Pending, when: &str) -> Vec<String> {
    let failed = |ok: bool| if ok { "" } else { " (WRITE FAILED)" };
    let ok = m.set_u32(p.addr.wrapping_add(layout::PEERS), p.peers);
    let mut lines = vec![format!(
        "session start: peer count back to {} {when}{}",
        p.peers,
        failed(ok)
    )];
    if let Some(n) = p.players {
        let ok = m.set_u32(p.addr.wrapping_add(layout::PLAYERS), n);
        lines.push(format!(
            "session start: player count back to {n} {when}{}",
            failed(ok)
        ));
    }
    lines
}

/// Ticks between two setup attempts, and attempts in all.
const SETUP_EVERY: u32 = 60;
const SETUP_ATTEMPTS: u32 = 3;

fn lobby_setup(
    st: &Arc<Mutex<State>>,
    want: &Want,
    max: u32,
    c: &mut Call,
) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (s, tick) = {
        let mem = rt.memory();
        (read_session(&*mem), game_tick(&*mem))
    };
    let Some(s) = s else {
        return Ok(Flow::Continue);
    };
    {
        let mut state = lock(st);
        if state.setup_for.as_ref() != Some(want) {
            state.setup_for = Some(want.clone());
            state.setup_attempts = 0;
            state.setup_last_tick = None;
            state.setup_done = false;
            state.setup_wait = None;
        }
        match host_setup(&s, want, max) {
            Setup::Wait(why) => {
                if state.setup_wait.as_deref() != Some(why.as_str()) && s.can_set_params() {
                    say(&format!("host: lobby setup: {why}"));
                }
                state.setup_wait = Some(why);
                state.setup_done = false;
                return Ok(Flow::Continue);
            }
            Setup::Done => {
                if !state.setup_done {
                    state.setup_done = true;
                    say(&format!(
                        "host: the session holds {} (campaign {}, map {}, difficulty {}); press Start in the lobby",
                        want.name, want.campaign, want.map, want.difficulty
                    ));
                }
                state.setup_wait = None;
                return Ok(Flow::Continue);
            }
            Setup::Due => {}
        }
        state.setup_done = false;
        if state.setup_attempts >= SETUP_ATTEMPTS {
            return Ok(Flow::Continue);
        }
        if let (Some(last), Some(now)) = (state.setup_last_tick, tick)
            && now.wrapping_sub(last) < SETUP_EVERY
        {
            return Ok(Flow::Continue);
        }
        state.setup_attempts += 1;
        state.setup_last_tick = tick;
        say(&format!(
            "host: setting the lobby to {} (campaign {}, map {}, difficulty {}), attempt {} of {SETUP_ATTEMPTS}",
            want.name, want.campaign, want.map, want.difficulty, state.setup_attempts
        ));
    }
    let saved = *c.cpu;
    let calls = setup_calls(want).to_vec();
    match run_calls(c.cpu, c.ctx, calls, saved) {
        Ok(t) => Ok(Flow::Return(t)),
        Err(e) => {
            *c.cpu = saved;
            say(&format!("host: the lobby setter call faulted: {e:?}"));
            Ok(Flow::Continue)
        }
    }
}

/// Makes `calls` in order, then re-enters the tick with the registers it
/// was entered with (its chain runs again, this time continuing).
fn run_calls(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut calls: Vec<SetupCall>,
    saved: CpuAbi,
) -> Result<TransferAbi, GuestFault> {
    if calls.is_empty() {
        *cpu = saved;
        return Ok(TransferAbi::jump(points::GAME_TICK.base));
    }
    let call = calls.remove(0);
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    *cpu = saved;
    for a in call.args.iter().rev().flatten() {
        cpu.esp = cpu.esp.wrapping_sub(4);
        mem.set_u32(cpu.esp, *a)?;
    }
    if let Some(v) = call.eax {
        cpu.eax = v;
    }
    if let Some(v) = call.esi {
        cpu.esi = v;
    }
    ctx.call_guest(cpu, &mut mem, call.point.base, move |cpu, ctx| {
        let ok = cpu.eax & 0xff != 0;
        say(&format!(
            "host: {} returned {}",
            call.point.name,
            if ok { "1" } else { "0 (refused)" }
        ));
        if !ok {
            *cpu = saved;
            return Ok(TransferAbi::jump(points::GAME_TICK.base));
        }
        run_calls(cpu, ctx, calls, saved)
    })
}

fn on_tick(st: &Mutex<State>, g: &runtime::Guest) {
    let tick = game_tick(g);
    let session = read_session(g).map(|s| s.describe());
    let sim = read_sim(g);
    let mut s = lock(st);
    if session != s.last_session {
        match &session {
            Some(line) => say(line),
            None if s.last_session.is_some() => say("no squad session"),
            None => {}
        }
        s.last_session = session;
    }
    if let Some(now) = sim {
        let lines = sim_changes(s.last_sim, now, tick);
        for l in &lines {
            say(l);
            if let (Some(t), Some(log)) = (tick, s.log.as_mut()) {
                log.note(&format!("tick {t} {l}"));
                let _ = log.out.flush();
            }
        }
        s.last_sim = Some(now);
    }
    let now = Instant::now();
    if s.log.is_some()
        && let Some(at) = s.last_tick_at
    {
        let ms = now.duration_since(at).as_millis();
        if ms > STALL_MS {
            let update = sim.map_or(0, |x| x.update);
            let msg = format!(
                "stall: {ms} ms without a tick before tick {} (last {}, update {update})",
                tick.map_or(-1, |t| t as i64),
                s.last_tick.map_or(-1, |t| t as i64),
            );
            say(&msg);
            if let (Some(t), Some(log)) = (tick, s.log.as_mut()) {
                log.note(&format!("tick {t} {msg}"));
            }
        }
    }
    s.last_tick_at = Some(now);
    s.last_tick = tick;
    if let (Some(t), Some(log)) = (tick, s.log.as_mut())
        && let Err(e) = log.tick(g, t)
        && !s.log_error
    {
        say(&format!("cannot hash tick {t}: {e}"));
        s.log_error = true;
    }
}

fn on_start(st: &Mutex<State>, g: &runtime::Guest, dir: &Path) {
    let game = level_boot::active_game(g);
    let sim = read_sim(g);
    let mode = sim.map_or(u32::MAX, |s| s.mode);
    let (opt_mode, path) = game.clone().unwrap_or((u32::MAX, "?".into()));
    say(&format!(
        "game start: options mode {}, simulation mode {} ({}), {path}",
        opt_mode as i32,
        mode_name(mode),
        mode as i32
    ));
    let mut s = lock(st);
    if let Some(mut old) = s.log.take() {
        let _ = old.out.flush();
    }
    s.last_tick_at = None;
    s.log_error = false;
    if opt_mode == level_boot::MODE_MAINMENU {
        return;
    }
    let role = role(mode);
    let mut header = vec![
        format!("role {role}"),
        format!("simulation mode {} {}", mode as i32, mode_name(mode)),
        format!("options mode {}", opt_mode as i32),
        format!("scenario {path}"),
    ];
    if let Some(sess) = read_session(g) {
        header.push(sess.describe());
    }
    match HashLog::open(dir, role, &header) {
        Ok(log) => {
            say(&format!("hash log {}", log.path.display()));
            s.log = Some(log);
        }
        Err(e) => say(&format!("cannot open a hash log in {}: {e}", dir.display())),
    }
}

// ---- install ----

/// What [`install`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Off,
    Added {
        level: Option<String>,
    },
    /// An address the probe needs already has an override outside the plan.
    Refused(Vec<String>),
}

/// The probe's points that already have an override `existing` names.
pub fn occupied(existing: impl Fn(u32) -> Option<String>) -> Vec<String> {
    HOOKED
        .iter()
        .filter_map(|p| existing(p.base).map(|n| format!("{} ({:#010x}) has {n}", p.name, p.base)))
        .collect()
}

/// Adds the probe to `registry` when `config` is given. The level is
/// resolved against `disc` here, at boot; a level that does not resolve
/// leaves the host setup out (the rest still runs). Refuses, with a line,
/// when one of its addresses is already overridden outside the plan, so
/// the plan never meets it.
pub fn install(
    rt: &runtime::Runtime,
    registry: &mut hooks::Registry,
    config: Option<Config>,
    disc: &Path,
    data_root: &Path,
) -> anyhow::Result<Installed> {
    let Some(config) = config else {
        return Ok(Installed::Off);
    };
    let mut taken = Vec::new();
    rt.edit_dispatch(|d| taken = occupied(|a| d.override_name(a).map(str::to_owned)));
    if !taken.is_empty() {
        for t in &taken {
            say(&format!("refused: {t}"));
        }
        say("off: an address it needs is already overridden");
        return Ok(Installed::Refused(taken));
    }
    let want = match level_boot::resolve(disc, &config.map, config.difficulty)
        .map_err(|e| e.to_string())
        .and_then(|r| Want::from_request(&r))
    {
        Ok(w) => {
            say(&format!(
                "on; the host sets {} {} (campaign {}, map {})",
                w.name, config.difficulty, w.campaign, w.map
            ));
            Some(w)
        }
        Err(e) => {
            say(&format!(
                "on; H2_COOP_PROBE_MAP={}: {e}; this machine will not set the lobby's level",
                config.map
            ));
            None
        }
    };
    let level = want.as_ref().map(|w| w.name.clone());
    let dir = data_root.join("coop-probe");
    match registry.add(&Probe::armed(Arm::forced(want), dir)) {
        Ok(_) => Ok(Installed::Added { level }),
        Err(e) => anyhow::bail!("coop-probe: {e}"),
    }
}

/// Adds the co-op over Live hooks to `registry`, armed by the returned
/// [`Arm`] (off until a co-op session arms it), unless an address they need
/// is already overridden outside the plan. Hash logs go to
/// `<data>/coop-probe/` as the probe's do.
pub fn install_live(
    rt: &runtime::Runtime,
    registry: &mut hooks::Registry,
    data_root: &Path,
) -> anyhow::Result<Option<Arc<Arm>>> {
    let mut taken = Vec::new();
    rt.edit_dispatch(|d| taken = occupied(|a| d.override_name(a).map(str::to_owned)));
    if !taken.is_empty() {
        for t in &taken {
            eprintln!("coop-live: refused: {t}");
        }
        eprintln!("coop-live: off: an address it needs is already overridden");
        return Ok(None);
    }
    let arm = Arc::new(Arm::default());
    match registry.add(&Probe::armed(
        Arc::clone(&arm),
        data_root.join("coop-probe"),
    )) {
        Ok(true) => {
            // Players 3 to 16 (docs/live/live-coop.md section 12), on the
            // same arm.
            crate::coop_players::install(registry, Arc::clone(&arm))?;
            // Join in progress (section 14), on the same arm.
            crate::coop_jip::install(registry, Arc::clone(&arm))?;
            // The latency line a second (section 15).
            crate::coop_latency::install(registry, Arc::clone(&arm))?;
            Ok(Some(arm))
        }
        Ok(false) => Ok(None),
        Err(e) => anyhow::bail!("coop-live: {e}"),
    }
}

#[cfg(test)]
mod tests;
