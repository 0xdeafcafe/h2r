//! The System Link Create row and Live Custom Game row, followed by the
//! lobby setters. See docs/live/dedicated-host.md, "Playerless lobby".

use super::{Command, Config, Variant, unavailable};
use crate::{coop_probe, layout_glue::ReadGm, level_boot, telemetry};
use agent_proto::host_lobby::{EmptyResult, OpenResult, Phase, Player, StateResult};
use agent_proto::{RpcError, code};
use bots::lobby;
use bots::phantom::session::{self, at::*};
use engine::entry::{Cx, GuestCall};
use engine::{GuestMemory, MemFault};
use feature::points;
use live_proto::hopper::variant;
use serde_json::{Value, json};

mod checkpoint;

const CREATE_BROWSER_SQUAD: u32 = 0x0019_9df9;
const LIVE_MENU_PREPARE: u32 = 0x0019_9e2e;
const CREATE_SQUAD: u32 = 0x0005_9890;
/// Retail lifecycle teardown, also used before a checkpoint scenario load.
const LEAVE_SESSIONS: u32 = 0x0005_93e0;
const SET_VARIANT: u32 = 0x0019_a728;
const CONTROLLER_COUNTDOWN: u32 = 0x0019_a78e;
const SESSION_NETWORK_CLASS: u32 = 0x18;

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
    Applied,
    Open,
}

struct Owned {
    addr: u32,
    config: Config,
    setup: Setup,
    applied_variant: Option<Box<[u8; variant::SIZE]>>,
    prior_live_id: Option<String>,
    live_id: Option<String>,
    closing: bool,
}

#[derive(Default)]
pub(crate) struct Machine {
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

    fn maps_loaded(&self, m: &dyn GuestMemory) -> Result<bool, RpcError> {
        for peer in 0..self.peer_count {
            let p = self.addr + PEERS + peer * PEER_SIZE;
            let status = m.u32(p + PEER_MAP_STATUS).map_err(read_fault)?;
            if !matches!(status, 3 | 4) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn phase(
    cycle: u32,
    stage: u32,
    countdown: bool,
    mode: Option<u32>,
    world: Option<u32>,
    ended: bool,
) -> Result<Phase, RpcError> {
    if cycle == 4 || (cycle == 3 && mode == Some(level_boot::MODE_MULTIPLAYER) && ended) {
        return Ok(Phase::Postgame);
    }
    if cycle == 3 && mode == Some(level_boot::MODE_MULTIPLAYER) {
        return match world {
            Some(session::WORLD_DIST_SERVER) => Ok(Phase::Playing),
            None | Some(0) => Ok(Phase::Loading),
            _ => Err(unavailable(
                "the active simulation is not the distributed server",
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
    pub(super) fn observe(&self, m: &dyn GuestMemory) -> Result<(StateResult, bool), RpcError> {
        let mode = level_boot::active_mode(&ReadGm(m));
        let idle = if mode == Some(level_boot::MODE_MAINMENU) {
            Phase::Menu
        } else {
            Phase::Loading
        };
        let empty = |phase| {
            (
                StateResult {
                    phase,
                    ..Default::default()
                },
                false,
            )
        };
        let Some(owner) = &self.owned else {
            return Ok(empty(idle));
        };
        if no_network_squad(m)? {
            if owner.closing || idle == Phase::Menu {
                return Ok(empty(idle));
            }
            return Err(unavailable("the dedicated squad disappeared"));
        }
        if owner.setup == Setup::Created
            && squad(m)? == Some(owner.addr)
            && m.u32(owner.addr + STATE).map_err(read_fault)? != session::STATE_LIVE
        {
            return Ok(empty(Phase::Loading));
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
        let mut phase = phase(s.cycle, s.stage, s.countdown, mode, world, ended)?;
        if phase == Phase::Playing {
            // A distributed world exists while the host still waits for
            // remote simulation views. States 3 and 4 precede running (5).
            let sim = m.u32(SIM_GLOBALS).map_err(read_fault)?;
            if m.u32(sim + SIM_STATE).map_err(read_fault)? != 5 {
                phase = Phase::Loading;
            }
        }
        let eligible = phase == Phase::Lobby
            && owner.setup == Setup::Open
            && !owner.closing
            && s.leader == s.local
            && s.map == owner.config.map
            && s.protocol == 2
            && s.max == u32::from(owner.config.max_players)
            && !players.is_empty()
            && players.len() <= usize::from(owner.config.max_players)
            && s.maps_loaded(m)?
            && replicated_variant(m, s.addr, owner)?;
        Ok((
            StateResult {
                phase,
                players,
                ready: false,
            },
            eligible,
        ))
    }

    pub(super) async fn state(&self, cx: &Cx) -> Result<StateResult, RpcError> {
        let (mut state, eligible) = cx.read(|m| self.observe(m))?;
        if eligible {
            state.ready = call(cx, GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
                .await?
                == lobby::READY;
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
            Command::Open(config) => self.open(cx, config, scratch, advertised, &[]).await,
            Command::Start => self.start(cx, machine_start).await,
            Command::Close => self.close(cx, advertised).await,
        }
    }

    /// A sim has one player per remote machine and a fixed, resolved variant.
    pub(crate) async fn open_sim(
        &mut self,
        cx: &Cx,
        map: u32,
        matched: &sim::config::Match,
        scratch: u32,
    ) -> Result<bool, RpcError> {
        let config = Config {
            live: false,
            map,
            max_players: matched.bots.len() as u8,
            variant: Variant::Stock(matched.game_type),
        };
        Ok(matches!(
            self.open(cx, &config, scratch, None, &matched.variant)
                .await?,
            Progress::Done(_)
        ))
    }

    pub(crate) async fn start_sim(&mut self, cx: &Cx, seats: usize) -> Result<bool, RpcError> {
        let state = self.state(cx).await?;
        if !state.ready || state.players.len() != seats {
            return Ok(false);
        }
        self.start_ready(cx, true).await?;
        Ok(true)
    }

    async fn open(
        &mut self,
        cx: &Cx,
        config: &Config,
        scratch: u32,
        advertised: Option<&str>,
        settings: &[(u32, i32)],
    ) -> Result<Progress, RpcError> {
        if let Some(owner) = &self.owned {
            if owner.config != *config || owner.closing {
                return Err(RpcError::new(
                    code::BUSY,
                    "close the current dedicated lobby first",
                ));
            }
        } else {
            let addr = cx.read(|m| {
                if level_boot::active_mode(&ReadGm(m)) != Some(level_boot::MODE_MAINMENU)
                    || m.u8(lobby::LOBBY_ON).map_err(read_fault)? == 0
                {
                    return Ok(None);
                }
                profiles(m)?;
                let Some(addr) = squad(m)? else {
                    return Ok(None);
                };
                if m.u32(addr + STATE).map_err(read_fault)? != 0
                    && m.u32(addr + SESSION_NETWORK_CLASS).map_err(read_fault)? != 0
                {
                    return Err(unavailable("a network squad already exists"));
                }
                Ok::<_, RpcError>(Some(addr))
            })?;
            let Some(addr) = addr else {
                return Ok(Progress::Wait("waiting for the main menu and networking"));
            };
            let prior_live_id = advertised.map(str::to_owned);
            let create = if config.live {
                call(cx, GuestCall::new(LIVE_MENU_PREPARE).args([1])).await?;
                GuestCall::new(CREATE_SQUAD).args([2, 0, 0])
            } else {
                GuestCall::new(CREATE_BROWSER_SQUAD).args([0, 0])
            };
            if call(cx, create).await? as u8 == 0 {
                return Err(unavailable(
                    "retail refused creation of the playerless squad",
                ));
            }
            // Create may replace the menu's offline session object.
            let addr = cx.read(|m| squad(m))?.unwrap_or(addr);
            self.owned = Some(Owned {
                addr,
                config: config.clone(),
                setup: Setup::Created,
                applied_variant: None,
                prior_live_id,
                live_id: None,
                closing: false,
            });
            call(cx, GuestCall::new(points::LOBBY_MODE_SET.base).args([2])).await?;
        }
        let owner = self.owned.as_mut().expect("created a squad");
        let waiting = cx.read(|m| {
            if squad(m)? != Some(owner.addr) {
                return Err(unavailable("the dedicated squad changed during open"));
            }
            m.u32(owner.addr + STATE).map_err(read_fault)
        })?;
        if waiting != session::STATE_LIVE {
            return Ok(Progress::Wait(
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
                Ok(Progress::Wait("waiting for the retail pregame life cycle"))
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
            return Ok(Progress::Wait("waiting for the retail pregame lobby"));
        }
        if owner.setup == Setup::Created {
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
            }
            if !settings.is_empty() {
                let mut raw = [0; variant::SIZE];
                cx.read(|m| m.read(scratch, &mut raw)).map_err(read_fault)?;
                let v = level_boot::configured_variant(&raw, settings).map_err(unavailable)?;
                cx.write(|m| m.write(scratch, &v.to_bytes()))
                    .map_err(read_fault)?;
            }
            let max = u32::from(config.max_players);
            cx.write(|m| m.set_u32(scratch + variant::MAX_PLAYERS as u32, max))
                .map_err(read_fault)?;
            if call(cx, GuestCall::new(SET_VARIANT).args([scratch])).await? as u8 == 0 {
                return Err(unavailable("retail refused the selected variant"));
            }
            let mut applied = Box::new([0; variant::SIZE]);
            cx.read(|m| m.read(scratch, applied.as_mut()))
                .map_err(read_fault)?;
            // Advertise the cap in the same tick as the variant. A remote
            // join on the next tick must see the requested number of seats.
            if cx
                .read(|m| m.u32(s.addr + MAX_PLAYERS))
                .map_err(read_fault)?
                != max
            {
                cx.write(|m| set_capacity(m, s.addr, max))?;
            }
            owner.applied_variant = Some(applied);
            owner.setup = Setup::Applied;
            return Ok(Progress::Wait("checking replicated lobby parameters"));
        }
        if s.map != config.map || s.protocol != 2 {
            return Ok(Progress::Wait(
                "waiting for the requested map and distributed protocol",
            ));
        }
        if !cx.read(|m| replicated_variant(m, s.addr, owner))? {
            return Ok(Progress::Wait(
                "waiting for the requested variant in the squad",
            ));
        }
        let max = u32::from(config.max_players);
        if s.max != max {
            cx.write(|m| set_capacity(m, s.addr, max))?;
            return Ok(Progress::Wait("checking the squad's player limit"));
        }
        if config.live {
            let Some(id) = advertised.filter(|id| Some(*id) != owner.prior_live_id.as_deref())
            else {
                return Ok(Progress::Wait("Live has not advertised the new squad"));
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
        Ok(Progress::Done(json!(OpenResult {
            session_id: owner.live_id.clone()
        })))
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
        call(cx, GuestCall::new(0x0005_a220).args([2])).await?;
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
            if addr != owner.addr || class != owner.config.class() {
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
