//! The System Link Create row and Live Custom Game row, followed by the
//! lobby setters. See docs/live/dedicated-host.md, "Playerless lobby".

use super::{Command, Config, Variant, unavailable};
use crate::variant_ext::SESSION_NETWORK_CLASS;
use crate::{coop_probe, layout_glue::ReadGm, level_boot, telemetry};
use agent_proto::host_lobby::{EmptyResult, OpenResult, Phase, Player, StateResult};
use agent_proto::{RpcError, code};
use bots::lobby;
use bots::phantom::session::{self, at::*};
use guest_layout::entry::{Cx, GuestCall};
use guest_layout::{GuestMemory, MemFault};
use hooks::points;
use live_proto::hopper::variant;
use serde_json::{Value, json};

mod checkpoint;

const CREATE_SQUAD: u32 = 0x0005_9890;
/// Retail lifecycle teardown, also used before a checkpoint scenario load.
pub(crate) const LEAVE_SESSIONS: u32 = 0x0005_93e0;
/// The lobby variant setter (`points::LOBBY_SET_VARIANT`): stack the
/// `0x130`-byte record; AL nonzero when taken. The pause menu's Change
/// Rules and Quick Options set the next game's variant with it
/// (`crate::pause_change_map`), and so does the lobby's Quick Options for
/// a script variant (`crate::variant_quick::page`).
pub(crate) static SET_VARIANT: &points::Point = &points::LOBBY_SET_VARIANT;
/// The controller countdown (`0x19a78e`): EAX the pressing controller's
/// port, stack 10 and 3 as the lobby's Start Game asks; AL nonzero when the
/// countdown started. The pause menu's Game Setup starts the next game
/// with it (`crate::pause_change_map`).
pub(crate) const CONTROLLER_COUNTDOWN: u32 = 0x0019_a78e;
/// The pregame stage change the countdown finisher (`0x6e410`) makes once
/// every peer has loaded the map: `0x6e4d7` calls it with the session in
/// ESI and the new stage in EDI, and it returns with a plain `ret`.
const MACHINE_LOAD_REQUEST: u32 = 0x0005_a220;
/// Session `+0x4980` after the load request: the machines load the map.
const STAGE_LOAD: u32 = 2;
const WORLD_SYNC_SERVER: u32 = 2;

pub(super) enum Progress {
    Done(Value),
    Wait(&'static str),
}

fn done() -> Progress {
    Progress::Done(json!(EmptyResult {}))
}

fn read_fault(e: MemFault) -> RpcError {
    unavailable(format!(
        "lobby guest memory at {:#010x}, {} bytes",
        e.addr, e.len
    ))
}

async fn call(cx: &Cx, f: GuestCall) -> Result<u32, RpcError> {
    let target = f.target;
    cx.call(f)
        .await
        .map(|r| r.eax)
        .map_err(|e| unavailable(format!("lobby call {target:#010x}: {e}")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setup {
    Created,
    Pregame,
    Applied,
    Open,
}

/// The title reuses its squad allocation after leave. The incarnation
/// changes on teardown, even when the next squad has the same address.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SquadId {
    addr: u32,
    incarnation: u32,
    class: u32,
}

impl SquadId {
    /// A failed read leaves ownership unknown until the next tick.
    pub(crate) fn is_current(self, m: &dyn GuestMemory) -> Option<bool> {
        if squad(m).ok()? != Some(self.addr) {
            return Some(false);
        }
        Some(
            m.u32(self.addr + INCARNATION).ok()? == self.incarnation
                && m.u32(self.addr + SESSION_NETWORK_CLASS).ok()? == self.class
                && matches!(self.class, 1 | 2),
        )
    }

    pub(super) fn playerless_host(self, m: &dyn GuestMemory) -> bool {
        self.is_current(m) == Some(true)
            && m.u32(self.addr + STATE) == Ok(session::STATE_LIVE)
            && m.u32(self.addr + LOCAL_PEER).is_ok_and(|local| {
                local < session::MAX && m.u32(self.addr + HOST_PEER) == Ok(local)
            })
            && profiles(m).is_ok()
    }
}

struct Owned {
    addr: u32,
    incarnation: u32,
    config: Config,
    setup: Setup,
    applied_variant: Option<Box<[u8; variant::SIZE]>>,
    prior_live_id: Option<String>,
    live_id: Option<String>,
    closing: bool,
}

#[derive(Default)]
pub struct Machine {
    owned: Option<Owned>,
}

/// A null squad and an allocated squad in state zero both occur in the menu.
/// Validate the bounds of the fixed layout before adding its field offsets.
fn squad(m: &dyn GuestMemory) -> Result<Option<u32>, RpcError> {
    if m.u8(lobby::NETWORKING).map_err(read_fault)? == 0 {
        return Ok(None);
    }
    let addr = m.u32(lobby::SESSION).map_err(read_fault)?;
    if addr == 0 {
        return Ok(None);
    }
    addr.checked_add(0x7900)
        .ok_or_else(|| unavailable("invalid squad pointer"))?;
    Ok(Some(addr))
}

fn no_network_squad(m: &dyn GuestMemory) -> Result<bool, RpcError> {
    let Some(addr) = squad(m)? else {
        return Ok(true);
    };
    Ok(m.u32(addr + STATE).map_err(read_fault)? == 0
        || m.u32(addr + SESSION_NETWORK_CLASS).map_err(read_fault)? == 0)
}

fn replicated_variant(m: &dyn GuestMemory, addr: u32, owner: &Owned) -> Result<bool, RpcError> {
    if owner.config.variant == Variant::Absent {
        // Engine 0 means no variant. A campaign difficulty would also
        // select protocol 1, so check the independent difficulty field.
        let engine = m.u32(addr + VARIANT + variant::ENGINE as u32).map_err(read_fault)?;
        let difficulty = m.u16(addr + coop_probe::layout::DIFFICULTY).map_err(read_fault)?;
        return Ok(engine == 0 && difficulty == coop_probe::NO_DIFFICULTY);
    }
    let Some(expected) = &owner.applied_variant else {
        return Ok(false);
    };
    let mut bytes = [0; variant::SIZE];
    m.read(addr + VARIANT, &mut bytes).map_err(read_fault)?;
    Ok(expected.as_ref() == &bytes)
}

fn profiles(m: &dyn GuestMemory) -> Result<(), RpcError> {
    for port in 0..4 {
        if bots::guest::signed_in(m, port).map_err(read_fault)? {
            return Err(unavailable(format!(
                "local profile signed in on port {port}"
            )));
        }
    }
    Ok(())
}

struct Squad {
    addr: u32,
    peer_count: u32,
    local: u32,
    leader: u32,
    map: u32,
    max: u32,
    protocol: u32,
    cycle: u32,
    stage: u32,
    countdown: bool,
}

impl Squad {
    fn read(m: &dyn GuestMemory, owner: &Owned) -> Result<Self, RpcError> {
        let addr = squad(m)?
            .filter(|&s| s == owner.addr)
            .ok_or_else(|| unavailable("the dedicated squad disappeared"))?;
        let get = |off| m.u32(addr + off).map_err(read_fault);
        if get(INCARNATION)? != owner.incarnation {
            return Err(unavailable("the dedicated squad was replaced"));
        }
        if get(STATE)? != session::STATE_LIVE {
            return Err(unavailable("squad is outside retail host state 5"));
        }
        let local = get(LOCAL_PEER)?;
        let peer_count = get(PEER_COUNT)?;
        if !(1..=session::MAX).contains(&peer_count)
            || local >= peer_count
            || get(HOST_PEER)? != local
        {
            return Err(unavailable("this machine no longer hosts the squad"));
        }
        let network_class = get(SESSION_NETWORK_CLASS)?;
        if network_class != owner.config.class() {
            return Err(unavailable(
                "the network class of the dedicated squad changed",
            ));
        }
        Ok(Self {
            addr,
            peer_count,
            local,
            leader: get(PEER_50)?,
            map: get(coop_probe::layout::MAP)?,
            max: get(MAX_PLAYERS)?,
            protocol: get(PROTOCOL)?,
            cycle: m.u32(lobby::SESSION_TYPE).map_err(read_fault)?,
            stage: get(CLASS)?,
            countdown: m.u8(addr + lobby::COUNTDOWN).map_err(read_fault)? != 0,
        })
    }

    fn roster(&self, m: &dyn GuestMemory) -> Result<Vec<Player>, RpcError> {
        let n = m.u32(self.addr + PLAYER_COUNT).map_err(read_fault)?;
        let mask = m.u32(self.addr + PLAYER_MASK).map_err(read_fault)?;
        if n != mask.count_ones() || mask > u16::MAX as u32 {
            return Err(unavailable("inconsistent session player count and mask"));
        }
        let mut players = Vec::with_capacity(n as usize);
        for slot in (0..session::MAX).filter(|i| mask & (1 << i) != 0) {
            let p = self.addr + PLAYERS + slot * PLAYER_SIZE;
            let peer = m.u32(p + P_PEER).map_err(read_fault)?;
            if peer >= self.peer_count || peer == self.local {
                return Err(unavailable(format!(
                    "player {slot} belongs to invalid or local peer {peer}"
                )));
            }
            let mut raw = [0; CONFIG_NAME_UNITS * 2];
            m.read(p + P_EFFECTIVE, &mut raw).map_err(read_fault)?;
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .take_while(|&u| u != 0)
                .collect();
            players.push(Player {
                slot,
                peer,
                name: String::from_utf16_lossy(&units),
            });
        }
        Ok(players)
    }

    /// Each peer's map status (`+0x88`), the local peer included.
    fn peer_maps(&self, m: &dyn GuestMemory) -> Result<Vec<u32>, RpcError> {
        (0..self.peer_count)
            .map(|peer| {
                m.u32(self.addr + PEERS + peer * PEER_SIZE + PEER_MAP_STATUS)
                    .map_err(read_fault)
            })
            .collect()
    }
}

/// The countdown finisher's map test (`0x6e4b8`): a signed status above 2
/// (3 and 4, loaded) on every peer. Retail's start check (`0x652a0`) lets
/// an unset status 0 through, the finisher does not, and the playerless
/// start takes the finisher's place.
fn map_loaded(status: u32) -> bool {
    status as i32 > 2
}

/// What a playerless lobby checks of its own before it asks retail's start
/// check (`0x19a2ce`). Read by [`Machine::observe`]; decided by
/// [`Eligibility::blocker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eligibility {
    pub phase: Phase,
    /// The lobby finished `open` and is not closing.
    pub open: bool,
    /// Session `+0x50` (party leader) and `+0x72d8` (this machine).
    pub leader: u32,
    pub local: u32,
    pub map: u32,
    pub want_map: u32,
    pub protocol: u32,
    pub want_protocol: u32,
    pub max: u32,
    pub want_max: u32,
    /// Session players, every one remote.
    pub players: usize,
    /// Each peer's map status, this machine's first when it is peer 0.
    pub peer_maps: Vec<u32>,
    /// The squad's `0x130`-byte rules still equal what open applied.
    pub variant: bool,
}

impl Eligibility {
    /// The first reason retail's start check is not worth asking yet, or
    /// `None` when the lobby is eligible.
    pub fn blocker(&self) -> Option<String> {
        if self.phase != Phase::Lobby {
            return Some(format!("the lobby is in phase {:?}", self.phase));
        }
        if !self.open {
            return Some("the lobby is not open (or is closing)".into());
        }
        if self.leader != self.local {
            return Some(format!(
                "this machine (peer {}) does not lead the party (leader {})",
                self.local as i32, self.leader as i32
            ));
        }
        if self.map != self.want_map {
            return Some(format!(
                "the squad's map is {} (want {})",
                self.map as i32, self.want_map
            ));
        }
        if self.protocol != self.want_protocol {
            return Some(format!(
                "the squad's protocol is {} (want {}, {})",
                self.protocol as i32, self.want_protocol, coop_probe::protocol_name(self.want_protocol)
            ));
        }
        if self.max != self.want_max {
            return Some(format!(
                "the squad's player limit is {} (want {})",
                self.max as i32, self.want_max
            ));
        }
        if self.players == 0 {
            return Some("no remote player has joined".into());
        }
        if self.players > self.want_max as usize {
            return Some(format!(
                "{} players exceed the limit {}",
                self.players, self.want_max
            ));
        }
        if let Some((peer, status)) = self
            .peer_maps
            .iter()
            .enumerate()
            .find(|(_, s)| !map_loaded(**s))
        {
            return Some(format!(
                "peer {peer} has map status {} (every peer needs 3 or 4; all: {:?})",
                *status as i32, self.peer_maps
            ));
        }
        if !self.variant {
            return Some("the squad's variant no longer matches the one applied".into());
        }
        None
    }
}

/// A playerless sim's start decision before retail's start check: the
/// lobby's own [`Eligibility::blocker`], then one remote player per seat.
pub fn sim_blocker(lobby: Option<String>, players: usize, seats: usize) -> Option<String> {
    if let Some(why) = lobby {
        return Some(why);
    }
    (players != seats).then(|| format!("{players} of {seats} seats have a player"))
}

/// Milestones acknowledged by retail while opening a playerless lobby.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SimOpen {
    Wait(&'static str),
    SquadCreated,
    SessionStarted,
    Ready,
}

/// What one playerless sim start attempt did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimStart {
    /// The machines were asked to load the map.
    Started,
    /// Not yet, and why.
    Wait(String),
}

fn phase(
    cycle: u32,
    stage: u32,
    countdown: bool,
    mode: Option<u32>,
    world: Option<u32>,
    host_world: u32,
    ended: bool,
) -> Result<Phase, RpcError> {
    if cycle == 4 || (cycle == 3 && mode == Some(level_boot::MODE_MULTIPLAYER) && ended) {
        return Ok(Phase::Postgame);
    }
    if cycle == 3 && mode == Some(level_boot::MODE_MULTIPLAYER) {
        return match world {
            Some(mode) if mode == host_world => Ok(Phase::Playing),
            None | Some(0) => Ok(Phase::Loading),
            _ => Err(unavailable(
                format!("the active simulation is not the {}", coop_probe::mode_name(host_world)),
            )),
        };
    }
    if matches!(cycle, 1..=3) {
        return Ok(if countdown || cycle == 3 || matches!(stage, 2..=4) {
            Phase::Loading
        } else {
            Phase::Lobby
        });
    }
    Err(unavailable(format!(
        "unexpected custom-game life cycle {cycle}"
    )))
}

impl Machine {
    pub(crate) fn owned_squad(&self) -> Option<SquadId> {
        self.owned.as_ref().filter(|o| !o.closing).map(|o| SquadId {
            addr: o.addr,
            incarnation: o.incarnation,
            class: o.config.class(),
        })
    }

    pub(super) fn advertisement(
        &self,
        cmd: &Command,
        live: Option<&live_hle::Hle>,
    ) -> Option<String> {
        let hle = live?;
        match cmd {
            Command::Open(config) if config.live => hle.lobby_hosted_session(),
            Command::Close => {
                let owner = self.owned.as_ref().filter(|o| o.config.live)?;
                if let Some(ours) = &owner.live_id {
                    // The current listing can change before ours is deleted.
                    // Confirm withdrawal of the actual ID returned by open.
                    return hle.hosts_session(ours).then(|| ours.clone());
                }
                hle.lobby_hosted_session()
            }
            _ => None,
        }
    }

    /// The previous RPC ended before retail acknowledged teardown. A new
    /// close can issue it again after checking the current squad's identity.
    pub(super) fn retry_close(&mut self) {
        if let Some(owner) = &mut self.owned {
            owner.closing = false;
        }
    }

    /// Read membership and lifecycle at an agent boundary, including a slice
    /// during a map load. Only an otherwise eligible lobby needs a guest call
    /// to distinguish retail's team/readiness checks from ready-to-start.
    pub(super) fn observe(
        &self,
        m: &dyn GuestMemory,
    ) -> Result<(StateResult, Option<String>), RpcError> {
        let mode = level_boot::active_mode(&ReadGm(m));
        let idle = if mode == Some(level_boot::MODE_MAINMENU) {
            Phase::Menu
        } else {
            Phase::Loading
        };
        let empty = |phase, why: &str| {
            (
                StateResult {
                    phase,
                    ..Default::default()
                },
                Some(why.to_owned()),
            )
        };
        let Some(owner) = &self.owned else {
            return Ok(empty(idle, "no dedicated lobby is open"));
        };
        if no_network_squad(m)? {
            if owner.closing || idle == Phase::Menu {
                return Ok(empty(idle, "no network squad"));
            }
            return Err(unavailable("the dedicated squad disappeared"));
        }
        if owner.setup == Setup::Created
            && squad(m)? == Some(owner.addr)
            && m.u32(owner.addr + STATE).map_err(read_fault)? != session::STATE_LIVE
        {
            return Ok(empty(Phase::Loading, "the squad is not in host state 5 yet"));
        }
        profiles(m)?;
        let s = Squad::read(m, owner)?;
        let players = s.roster(m)?;
        let world = coop_probe::read_sim(&ReadGm(m)).map(|sim| sim.mode);
        let ended = if mode == Some(level_boot::MODE_MULTIPLAYER) {
            let game = m.u32(points::GAME_GLOBALS.base).map_err(read_fault)?;
            m.u8(game + telemetry::layout::GAME_ENDED)
                .map_err(read_fault)?
                != 0
        } else {
            false
        };
        let host_world = if owner.config.protocol() == 1 {
            WORLD_SYNC_SERVER
        } else {
            session::WORLD_DIST_SERVER
        };
        let mut phase = phase(s.cycle, s.stage, s.countdown, mode, world, host_world, ended)?;
        if phase == Phase::Playing {
            // A server world exists while the host still waits for
            // remote simulation views. States 3 and 4 precede running (5).
            let sim = m.u32(SIM_GLOBALS).map_err(read_fault)?;
            if m.u32(sim + SIM_STATE).map_err(read_fault)? != 5 {
                phase = Phase::Loading;
            }
        }
        let blocker = Eligibility {
            phase,
            open: owner.setup == Setup::Open && !owner.closing,
            leader: s.leader,
            local: s.local,
            map: s.map,
            want_map: owner.config.map,
            protocol: s.protocol,
            want_protocol: owner.config.protocol(),
            max: s.max,
            want_max: u32::from(owner.config.max_players),
            players: players.len(),
            peer_maps: s.peer_maps(m)?,
            variant: replicated_variant(m, s.addr, owner)?,
        }
        .blocker();
        Ok((
            StateResult {
                phase,
                players,
                ready: false,
            },
            blocker,
        ))
    }

    pub(super) async fn state(&self, cx: &Cx) -> Result<StateResult, RpcError> {
        let (mut state, blocker) = cx.read(|m| self.observe(m))?;
        if blocker.is_none() {
            state.ready = start_state(cx).await? == lobby::READY;
        }
        Ok(state)
    }

    pub(super) async fn advance(
        &mut self,
        cx: &Cx,
        cmd: &Command,
        scratch: u32,
        advertised: Option<&str>,
        machine_start: bool,
    ) -> Result<Progress, RpcError> {
        match cmd {
            Command::Open(config) => Ok(
                match self.open(cx, config, scratch, advertised, &[]).await? {
                    SimOpen::Wait(why) => Progress::Wait(why),
                    SimOpen::SquadCreated => {
                        Progress::Wait("waiting for the retail pregame session")
                    }
                    SimOpen::SessionStarted => {
                        Progress::Wait("applying the requested map and variant")
                    }
                    SimOpen::Ready => Progress::Done(json!(OpenResult {
                        session_id: self.owned.as_ref().and_then(|o| o.live_id.clone()),
                    })),
                },
            ),
            Command::Start => self.start(cx, machine_start).await,
            Command::Close => self.close(cx, advertised).await,
        }
    }

    /// A sim has one player per remote machine and a fixed, resolved variant.
    pub async fn open_sim(
        &mut self,
        cx: &Cx,
        map: u32,
        matched: &sim_bench::config::Match,
        scratch: u32,
    ) -> Result<bool, RpcError> {
        Ok(self.advance_sim_open(cx, map, matched, scratch).await? == SimOpen::Ready)
    }

    pub(crate) async fn advance_sim_open(
        &mut self,
        cx: &Cx,
        map: u32,
        matched: &sim_bench::config::Match,
        scratch: u32,
    ) -> Result<SimOpen, RpcError> {
        let config = Config {
            live: false,
            map,
            max_players: matched.bots.len() as u8,
            variant: Variant::Stock(matched.game_type),
        };
        self.open(cx, &config, scratch, None, &matched.variant)
            .await
    }

    /// One start attempt for a playerless sim with `seats` phantoms: the
    /// lobby's own checks, then retail's start check, then the machine load
    /// request. A wait names what holds it back, for the run's log.
    pub async fn start_sim(&mut self, cx: &Cx, seats: usize) -> Result<SimStart, RpcError> {
        let (state, blocker) = cx.read(|m| self.observe(m))?;
        if let Some(why) = sim_blocker(blocker, state.players.len(), seats) {
            return Ok(SimStart::Wait(why));
        }
        let start = start_state(cx).await?;
        if start != lobby::READY {
            let why = cx.read(|m| match lobby::facts(m) {
                Ok(f) => lobby::blocked_by(start, &f),
                Err(e) => format!("the session read faulted at {:#x}", e.addr),
            });
            return Ok(SimStart::Wait(format!(
                "retail start state {start}, blocked by {why}"
            )));
        }
        self.start_ready(cx, true).await?;
        Ok(SimStart::Started)
    }

    async fn open(
        &mut self,
        cx: &Cx,
        config: &Config,
        scratch: u32,
        advertised: Option<&str>,
        settings: &[(u32, i32)],
    ) -> Result<SimOpen, RpcError> {
        if let Some(owner) = &self.owned {
            if owner.config != *config || owner.closing {
                return Err(RpcError::new(
                    code::BUSY,
                    "close the current dedicated lobby first",
                ));
            }
        } else {
            let waiting = cx.read(|m| {
                if level_boot::active_mode(&ReadGm(m)) != Some(level_boot::MODE_MAINMENU) {
                    return Ok(Some("waiting for the main menu"));
                }
                if m.u8(lobby::LOBBY_ON).map_err(read_fault)? == 0 {
                    return Ok(Some("waiting for the lobby subsystem"));
                }
                if m.u8(lobby::NETWORKING).map_err(read_fault)? == 0 {
                    return Ok(Some("waiting for networking"));
                }
                profiles(m)?;
                let Some(addr) = squad(m)? else {
                    return Ok(Some("waiting for the squad allocation"));
                };
                if m.u32(addr + STATE).map_err(read_fault)? != 0
                    && m.u32(addr + SESSION_NETWORK_CLASS).map_err(read_fault)? != 0
                {
                    return Err(unavailable("a network squad already exists"));
                }
                // Both Create paths first ask retail to leave the squad and
                // target. It dereferences both, even at the initial menu.
                let target = m.u32(lobby::TARGET_SESSION).map_err(read_fault)?;
                if target == 0 {
                    return Ok(Some("waiting for the target session allocation"));
                }
                let state = target
                    .checked_add(STATE)
                    .ok_or_else(|| unavailable("invalid target session"))?;
                if target != addr && m.u32(state).map_err(read_fault)? != 0 {
                    return Err(unavailable("another target session is active"));
                }
                Ok::<_, RpcError>(None)
            })?;
            if let Some(why) = waiting {
                return Ok(SimOpen::Wait(why));
            }
            let prior_live_id = advertised.map(str::to_owned);
            let create = if config.live {
                call(cx, GuestCall::new(points::UI_LEAVE_SESSIONS.base).args([1])).await?;
                GuestCall::new(CREATE_SQUAD).args([2, 0, 0])
            } else {
                GuestCall::new(points::BROWSER_CREATE_SQUAD.base).args([0, 0])
            };
            if call(cx, create).await? as u8 == 0 {
                return Err(unavailable(
                    "retail refused creation of the playerless squad",
                ));
            }
            // Create may replace the menu's offline session object.
            let (addr, incarnation) = cx.read(|m| {
                let addr =
                    squad(m)?.ok_or_else(|| unavailable("Create returned without a squad"))?;
                if m.u32(addr + STATE).map_err(read_fault)? == 0
                    || m.u32(addr + SESSION_NETWORK_CLASS).map_err(read_fault)? != config.class()
                {
                    return Err(unavailable(
                        "Create returned without the requested network squad",
                    ));
                }
                Ok((addr, m.u32(addr + INCARNATION).map_err(read_fault)?))
            })?;
            self.owned = Some(Owned {
                addr,
                incarnation,
                config: config.clone(),
                setup: Setup::Created,
                applied_variant: None,
                prior_live_id,
                live_id: None,
                closing: false,
            });
            call(cx, GuestCall::new(points::LOBBY_MODE_SET.base).args([2])).await?;
            return Ok(SimOpen::SquadCreated);
        }
        let owner = self.owned.as_mut().expect("created a squad");
        let waiting = cx.read(|m| {
            if squad(m)? != Some(owner.addr) {
                return Err(unavailable("the dedicated squad changed during open"));
            }
            m.u32(owner.addr + STATE).map_err(read_fault)
        })?;
        if waiting != session::STATE_LIVE {
            return Ok(SimOpen::Wait(
                "waiting for the retail squad to reach host state 5",
            ));
        }
        let s = cx.read(|m| Squad::read(m, owner))?;
        if s.leader != s.local {
            return Err(unavailable(
                "the machine hosts the squad but does not lead the party",
            ));
        }
        if !matches!(s.cycle, 1 | 2) {
            return if s.cycle == 0 {
                Ok(SimOpen::Wait("waiting for the retail pregame life cycle"))
            } else {
                Err(unavailable("squad is outside the pregame lobby"))
            };
        }
        let joined = cx.read(|m| {
            profiles(m)?;
            s.roster(m)
        })?;
        if joined.len() > usize::from(config.max_players) {
            return Err(unavailable("joined players exceed the requested limit"));
        }
        if s.stage != 1 || cx.read(|m| m.u8(lobby::LOBBY_ON)).map_err(read_fault)? == 0 {
            return Ok(SimOpen::Wait("waiting for the retail pregame lobby"));
        }
        if owner.setup == Setup::Created {
            owner.setup = Setup::Pregame;
            return Ok(SimOpen::SessionStarted);
        }
        if owner.setup == Setup::Pregame {
            if call(
                cx,
                GuestCall::new(points::LOBBY_SET_MAP.base)
                    .esi(config.map)
                    .args([u32::MAX]),
            )
            .await? as u8
                == 0
            {
                return Err(unavailable("retail refused the selected multiplayer map"));
            }
            let max = u32::from(config.max_players);
            if config.variant == Variant::Absent {
                // A new squad starts synchronous (0x00059d58). Use the
                // ordinary MP map setter and leave the variant untouched.
                let remains_synchronous = cx.read(|m| {
                    Ok::<_, RpcError>(m.u32(s.addr + PROTOCOL).map_err(read_fault)? == 1
                        && replicated_variant(m, s.addr, owner)?)
                })?;
                if !remains_synchronous {
                    return Err(unavailable("the multiplayer lobby did not retain a synchronous session with no variant"));
                }
            } else {
                match &config.variant {
                    Variant::Stock(t) => {
                        call(
                            cx,
                            GuestCall::new(level_boot::VARIANT_DEFAULTS)
                                .args([scratch, t.list_index()]),
                        )
                        .await?;
                    }
                    Variant::Named(v) => cx
                        .write(|m| m.write(scratch, v.bytes()))
                        .map_err(read_fault)?,
                    Variant::Absent => unreachable!("handled above"),
                }
                if !settings.is_empty() {
                    let mut raw = [0; variant::SIZE];
                    cx.read(|m| m.read(scratch, &mut raw)).map_err(read_fault)?;
                    let v = level_boot::configured_variant(&raw, settings).map_err(unavailable)?;
                    cx.write(|m| m.write(scratch, &v.to_bytes()))
                        .map_err(read_fault)?;
                }
                cx.write(|m| m.set_u32(scratch + variant::MAX_PLAYERS as u32, max))
                    .map_err(read_fault)?;
                if call(cx, GuestCall::new(SET_VARIANT.base).args([scratch])).await? as u8 == 0 {
                    return Err(unavailable("retail refused the selected variant"));
                }
                let mut applied = Box::new([0; variant::SIZE]);
                cx.read(|m| m.read(scratch, applied.as_mut()))
                    .map_err(read_fault)?;
                owner.applied_variant = Some(applied);
            }
            // Advertise the cap in the same tick as the variant. A remote
            // join on the next tick must see the requested number of seats.
            if cx
                .read(|m| m.u32(s.addr + MAX_PLAYERS))
                .map_err(read_fault)?
                != max
            {
                cx.write(|m| set_capacity(m, s.addr, max))?;
            }
            owner.setup = Setup::Applied;
            return Ok(SimOpen::Wait("checking replicated lobby parameters"));
        }
        if s.map != config.map || s.protocol != config.protocol() {
            return Ok(SimOpen::Wait(
                "waiting for the requested map and protocol",
            ));
        }
        if !cx.read(|m| replicated_variant(m, s.addr, owner))? {
            return Ok(SimOpen::Wait(
                "waiting for the requested variant in the squad",
            ));
        }
        let max = u32::from(config.max_players);
        if s.max != max {
            cx.write(|m| set_capacity(m, s.addr, max))?;
            return Ok(SimOpen::Wait("checking the squad's player limit"));
        }
        if config.live {
            let Some(id) = advertised.filter(|id| Some(*id) != owner.prior_live_id.as_deref())
            else {
                return Ok(SimOpen::Wait("Live has not advertised the new squad"));
            };
            if owner
                .live_id
                .as_deref()
                .is_some_and(|previous| previous != id)
            {
                return Err(unavailable("the dedicated Live advertisement changed"));
            }
            owner.live_id = Some(id.to_owned());
        }
        owner.setup = Setup::Open;
        Ok(SimOpen::Ready)
    }

    async fn start(&mut self, cx: &Cx, machine_start: bool) -> Result<Progress, RpcError> {
        if self
            .owned
            .as_ref()
            .is_none_or(|owner| owner.closing || owner.setup != Setup::Open)
        {
            return Err(unavailable("open a dedicated lobby before starting it"));
        }
        let state = self.state(cx).await?;
        if state.phase == Phase::Loading || state.phase == Phase::Playing {
            return Ok(done());
        }
        if !state.ready {
            return Err(unavailable(
                "start requires a ready pregame lobby with a remote player",
            ));
        }
        self.start_ready(cx, machine_start).await
    }

    async fn start_ready(&mut self, cx: &Cx, machine_start: bool) -> Result<Progress, RpcError> {
        // Preserve the ordinary path for the control run. The pressing
        // controller's local-user index is -1 on a playerless machine.
        if call(
            cx,
            GuestCall::new(CONTROLLER_COUNTDOWN).eax(0).args([10, 3]),
        )
        .await? as u8
            != 0
        {
            return Ok(done());
        }
        let index = bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER;
        if cx.read(|m| m.u32(index)).map_err(read_fault)? != u32::MAX {
            return Err(unavailable(
                "retail refused a countdown with a valid controller user",
            ));
        }
        if !machine_start {
            return Err(unavailable(
                "retail refused the countdown without a local owner profile",
            ));
        }
        let owner = self.owned.as_ref().expect("ready dedicated lobby");
        // The transition the pregame lifecycle itself requests when every
        // peer has loaded its map (0x6e410). Keep retail's view handshakes.
        // Registers, not the stack: ESI the session, EDI the stage.
        call(
            cx,
            GuestCall::new(MACHINE_LOAD_REQUEST)
                .esi(owner.addr)
                .edi(STAGE_LOAD),
        )
        .await?;
        let stage = cx.read(|m| m.u32(owner.addr + CLASS)).map_err(read_fault)?;
        if !matches!(stage, 2..=4) {
            return Err(unavailable("retail refused the machine's load request"));
        }
        Ok(done())
    }

    async fn close(&mut self, cx: &Cx, advertised: Option<&str>) -> Result<Progress, RpcError> {
        let Some(owner) = &mut self.owned else {
            return Ok(done());
        };
        let (network, in_menu) = cx.read(|m| {
            let in_menu = level_boot::active_mode(&ReadGm(m)) == Some(level_boot::MODE_MAINMENU);
            let Some(addr) = squad(m)? else {
                return Ok((false, in_menu));
            };
            let state = m.u32(addr + STATE).map_err(read_fault)?;
            let class = m.u32(addr + SESSION_NETWORK_CLASS).map_err(read_fault)?;
            if state == 0 || class == 0 {
                return Ok((false, in_menu));
            }
            if addr != owner.addr
                || class != owner.config.class()
                || m.u32(addr + INCARNATION).map_err(read_fault)? != owner.incarnation
            {
                return Err(unavailable("squad changed before close"));
            }
            if state == session::STATE_LIVE
                && m.u32(addr + HOST_PEER).map_err(read_fault)?
                    != m.u32(addr + LOCAL_PEER).map_err(read_fault)?
            {
                return Err(unavailable("the squad migrated before close"));
            }
            Ok::<_, RpcError>((true, in_menu))
        })?;
        let withdrawn = !owner.config.live
            || advertised.is_none()
            || advertised == owner.prior_live_id.as_deref();
        if !network && in_menu && withdrawn {
            self.owned = None;
            return Ok(done());
        }
        if !owner.closing {
            if network {
                // 0x593e0 retires both the squad and target. Never tear
                // down a separate matchmaking session on this machine.
                cx.read(|m| {
                    let target = m.u32(lobby::TARGET_SESSION).map_err(read_fault)?;
                    if target != 0 && target != owner.addr {
                        let at = target
                            .checked_add(STATE)
                            .ok_or_else(|| unavailable("invalid target session"))?;
                        if m.u32(at).map_err(read_fault)? != 0 {
                            return Err(unavailable("another target session is active"));
                        }
                    }
                    Ok(())
                })?;
                call(cx, GuestCall::new(LEAVE_SESSIONS)).await?;
            }
            if !in_menu {
                call(cx, GuestCall::new(level_boot::REQUEST_MAINMENU).args([0])).await?;
            }
            owner.closing = true;
        }
        Ok(Progress::Wait(
            "waiting for the network squad to close and the menu to return",
        ))
    }
}

/// Retail's start check (`0x19a2ce`, no progress out).
async fn start_state(cx: &Cx) -> Result<u32, RpcError> {
    call(cx, GuestCall::new(points::LOBBY_START_STATE.base).args([0])).await
}

fn set_capacity(m: &mut dyn GuestMemory, addr: u32, max: u32) -> Result<(), RpcError> {
    let count = m.u32(addr + PLAYER_COUNT).map_err(read_fault)?;
    if count > max {
        return Err(unavailable(
            "joined players already exceed the requested limit",
        ));
    }
    let version = addr + coop_probe::layout::PARAMS_VERSION;
    let revision = m.u32(version).map_err(read_fault)?;
    m.set_u32(addr + MAX_PLAYERS, max).map_err(read_fault)?;
    m.set_u32(version, revision.wrapping_add(1))
        .map_err(read_fault)
}

#[cfg(test)]
#[path = "retail_tests.rs"]
mod tests;
