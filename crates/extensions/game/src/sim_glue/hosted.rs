//! A headless host follows the System Link browser's Create Game path.
//! The playerless host waits for the registered phantom peers before starting.
use super::{Inner, Phase};
use crate::host_lobby::{PlayerlessLobby, SimOpen, SimStart, SquadId};
use crate::layout_glue::{Gm, ReadGm};
use crate::{coop_probe, level_boot};
use anyhow::{Result, bail};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use hooks::{Flow, points};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use sim_bench::metrics::SetupStage;
use std::sync::Arc;
use std::time::Instant;

/// Hold the eighth port of a block for the lifetime of this process. Retail
/// uses base through base+6, and discovery stays on this host's own endpoint.
pub struct Network {
    _lease: UdpSocket,
    pub config: live_hle::NetworkConfig,
}
impl Network {
    pub fn reserve() -> Result<Self> {
        for port in (24000..48000).step_by(8) {
            let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
            let Ok(lease) = UdpSocket::bind((Ipv4Addr::LOCALHOST, port + 7)) else {
                continue;
            };
            if !(port..=port + 6).all(|p| UdpSocket::bind((Ipv4Addr::LOCALHOST, p)).is_ok()) {
                continue;
            }
            let mut config = live_hle::NetworkConfig::new(addr);
            config.loopback_peers.push(addr);
            return Ok(Self {
                _lease: lease,
                config,
            });
        }
        bail!("sim: no free loopback XNet port block")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Create,
    SquadCreated,
    SessionStarted,
    Start,
    Loading,
}

/// A sample at the same guest boundary that supplied the wait or acknowledgement.
#[derive(Default)]
struct Facts {
    main_menu: bool,
    networking: bool,
    lobby_on: bool,
    session_type: Option<u32>,
    squad: Option<u32>,
    squad_state: Option<u32>,
    network_class: Option<u32>,
}

impl Facts {
    fn read(m: &impl Gm) -> Self {
        let networking = m.u8(points::NETWORK_READY.base).is_some_and(|r| r != 0);
        let squad = networking
            .then(|| m.u32(points::SQUAD_SESSION.base))
            .flatten();
        let field = |off| {
            squad
                .filter(|&p| p != 0)?
                .checked_add(off)
                .and_then(|p| m.u32(p))
        };
        Self {
            main_menu: level_boot::active_mode(m) == Some(level_boot::MODE_MAINMENU),
            networking,
            lobby_on: m.u8(bots::lobby::LOBBY_ON).is_some_and(|r| r != 0),
            session_type: m.u32(bots::lobby::SESSION_TYPE),
            squad,
            squad_state: field(bots::lobby::STATE),
            network_class: field(crate::variant_ext::SESSION_NETWORK_CLASS),
        }
    }

    /// The setup stage this sample shows the run has reached
    /// (`wall.setup`): at the menu, then able to open a lobby.
    fn setup_stage(&self) -> Option<SetupStage> {
        if self.wait().is_none() {
            Some(SetupStage::Lobby)
        } else if self.main_menu {
            Some(SetupStage::Menu)
        } else {
            None
        }
    }

    fn wait(&self) -> Option<&'static str> {
        if !self.main_menu {
            Some("waiting for the main menu")
        } else if !self.networking {
            Some("waiting for networking")
        } else if !self.lobby_on {
            Some("waiting for the lobby subsystem")
        } else {
            None
        }
    }

    /// An allocated menu object may remain at the same address after teardown.
    /// A replacement releases ownership too, even if its address was reused.
    fn lost_after(&self, step: Step, current: Option<bool>) -> bool {
        matches!(
            step,
            Step::SquadCreated | Step::SessionStarted | Step::Start
        ) && (self.squad == Some(0)
            || self.squad_state == Some(0)
            || self.network_class == Some(0)
            || current == Some(false))
    }
}

pub(super) struct LobbyState {
    step: Step,
    in_tick: bool,
    playerless: Option<PlayerlessLobby>,
    pub(super) owned_squad: Option<SquadId>,
    waiting: Option<String>,
    facts: Facts,
}

impl Default for LobbyState {
    fn default() -> Self {
        Self {
            step: Step::Create,
            in_tick: false,
            playerless: Some(PlayerlessLobby::default()),
            owned_squad: None,
            waiting: None,
            facts: Facts::default(),
        }
    }
}

/// What the host's squad must show before `Step::Start` asks to start:
/// live (state 5), hosted here, distributed, on the match's map, with one
/// player per seat and one peer per machine (this host's own and each
/// phantom's). `None`: ready to try. The same for a playerless host and a
/// mixed roster.
fn start_gate(
    s: Option<&coop_probe::Session>,
    cycle: Option<u32>,
    map: u32,
    seats: usize,
    machines: usize,
) -> Option<String> {
    let Some(s) = s else {
        return Some("could not read the hosted squad".into());
    };
    if s.state != 5 {
        return Some(format!("squad state {} (want 5)", s.state));
    }
    match cycle {
        Some(1 | 2) => (),
        Some(cycle) => return Some(format!("session type {cycle} (want pregame 1 or 2)")),
        None => return Some("could not read the session type".into()),
    }
    if !s.is_host() {
        return Some("this machine does not host the squad".into());
    }
    if s.protocol != 2 {
        return Some(format!(
            "protocol {} (want 2, distributed)",
            s.protocol as i32
        ));
    }
    if s.map != map {
        return Some(format!("map {} (want {map})", s.map as i32));
    }
    if s.players != seats as u32 {
        return Some(format!("{} of {seats} players", s.players));
    }
    if s.peers != machines as u32 {
        return Some(format!(
            "{} of {machines} peers (this host and one per phantom)",
            s.peers
        ));
    }
    None
}

impl LobbyState {
    fn update(&mut self, step: Step, why: Option<String>, facts: Facts) {
        if step != self.step {
            eprintln!("sim: System Link host {:?} -> {step:?}", self.step);
        }
        if self.waiting != why || self.step != step {
            if let Some(w) = &why {
                eprintln!("sim: System Link host waits at {step:?}: {w}");
            } else if self.step == step && self.waiting.is_some() {
                eprintln!("sim: System Link host {step:?}: no longer waiting");
            }
        }
        self.step = step;
        self.waiting = why;
        self.facts = facts;
    }

    pub(super) fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "step": format!("{:?}", self.step),
            "wait": self.waiting,
            "session_type": self.facts.session_type,
            "squad_state": self.facts.squad_state,
        })
    }

    pub(super) fn setup_complete(&self) -> bool {
        self.step == Step::Loading && !self.in_tick
    }

    pub(super) fn resume_match(&mut self, owner: Option<SquadId>) {
        self.owned_squad = owner;
        self.playerless = None;
        self.step = Step::Loading;
        self.in_tick = false;
        self.waiting = None;
        self.facts = Facts::default();
    }
}

pub(super) fn install(b: &mut hooks::Builder, sim: Arc<Inner>) -> Result<(), hooks::BuildError> {
    let buffer = b.guest_alloc(0x1000, runtime::guest::Permissions::RW)?;
    // Every bot is a phantom peer, so the host console plays no local
    // player of its own: its lobby is playerless and no local profile is
    // offered.
    let stage = sim.lobby.clone();
    let guard = stage.clone();
    crate::host_lobby::playerless_hooks(b, move || {
        guard.lock().unwrap_or_else(|e| e.into_inner()).owned_squad
    });
    b.raw()
        .before(&points::GAME_TICK, "sim: System Link host", move |c| {
            if sim.lock().phase != Phase::Waiting {
                return Ok(Flow::Continue);
            }
            let g = c.ctx.runtime().memory();
            let mut state = stage.lock().unwrap_or_else(|e| e.into_inner());
            let step = state.step;
            if state.in_tick || step == Step::Loading {
                return Ok(Flow::Continue);
            }
            let facts = Facts::read(&*g);
            if let Some(stage) = facts.setup_stage() {
                sim.lock().setup.enter(stage, Instant::now());
            }
            let Some(req) = sim.hosted_map.get() else {
                state.update(step, Some("waiting for the match map".into()), facts);
                return Ok(Flow::Continue);
            };
            let map = req.map;
            if let Some(why) = facts.wait() {
                state.update(step, Some(why.into()), facts);
                return Ok(Flow::Continue);
            }
            let current = state.owned_squad.and_then(|id| id.is_current(&*g));
            if facts.lost_after(step, current) {
                state.playerless = Some(PlayerlessLobby::default());
                state.owned_squad = None;
                state.update(
                    Step::Create,
                    Some("the hosted squad retired or was replaced".into()),
                    facts,
                );
                return Ok(Flow::Continue);
            }
            if state.owned_squad.is_some() && current.is_none() {
                state.update(step, Some("the owned squad is unreadable".into()), facts);
                return Ok(Flow::Continue);
            }
            if step == Step::Start {
                let seats = sim.matched.bots.len();
                let session = coop_probe::read_session(&*g);
                let why = start_gate(session.as_ref(), facts.session_type, map, seats, seats + 1);
                if why.is_some() {
                    state.update(step, why, facts);
                    return Ok(Flow::Continue);
                }
            }
            drop(g);
            let mut lobby = state
                .playerless
                .take()
                .expect("lobby present outside its tick");
            state.in_tick = true;
            drop(state);
            let sim = sim.clone();
            let stage = stage.clone();
            c.answer_async(move |cx| {
                Box::pin(async move {
                    let result = advance(&cx, &sim, step, map, buffer, &mut lobby).await;
                    let facts = cx.read(|m| Facts::read(&ReadGm(m)));
                    let reached = result.as_ref().ok().and_then(|(next, why)| result_stage(*next, why.as_deref()));
                    if let Some(reached) = reached {
                        sim.lock().setup.enter(reached, Instant::now());
                    }
                    {
                        let mut state = stage.lock().unwrap_or_else(|e| e.into_inner());
                        state.owned_squad = lobby.owned_squad();
                        state.playerless = Some(lobby);
                        match result {
                            Ok((next, why)) => state.update(next, why, facts),
                            Err(e) => {
                                state.update(step, Some(e.to_string()), facts);
                                state.in_tick = false;
                                return Err(e);
                            }
                        }
                    }
                    // Publish before retail runs: the load can stop menu ticks.
                    // The guard lets the reentered tick run exactly once.
                    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
                    stage.lock().unwrap_or_else(|e| e.into_inner()).in_tick = false;
                    Ok(Ret::new(r?.eax, 0))
                })
            })
        })
        .last();
    Ok(())
}

async fn advance(
    cx: &Cx,
    sim: &Inner,
    step: Step,
    map: u32,
    buffer: u32,
    lobby: &mut PlayerlessLobby,
) -> Result<(Step, Option<String>), EntryError> {
    match step {
        Step::Create | Step::SquadCreated | Step::SessionStarted => {
            let open = lobby
                .advance_sim_open(cx, map, &sim.matched, buffer)
                .await
                .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
            Ok(open_step(step, open))
        }
        Step::Start => {
            let started = lobby
                .start_sim(cx, sim.matched.bots.len())
                .await
                .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
            Ok(match started {
                SimStart::Started => (Step::Loading, None),
                SimStart::Wait(why) => (step, Some(why)),
            })
        }
        Step::Loading => Ok((step, None)),
    }
}

/// The setup stage a lobby step's result starts: the first wait on a peer's
/// map status begins the copy wait, the start request being accepted begins
/// the load.
fn result_stage(next: Step, why: Option<&str>) -> Option<SetupStage> {
    match (next, why) {
        (Step::Loading, _) => Some(SetupStage::Load),
        (Step::Start, Some(why)) if why.contains(" has map status ") => Some(SetupStage::CopyWait),
        _ => None,
    }
}

fn open_step(step: Step, progress: SimOpen) -> (Step, Option<String>) {
    match progress {
        SimOpen::Wait(why) => (step, Some(why.into())),
        SimOpen::SquadCreated => (Step::SquadCreated, None),
        SimOpen::SessionStarted => (Step::SessionStarted, None),
        SimOpen::Ready => (Step::Start, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The playerless host's squad: hosted here (peer 0), distributed, on
    /// `map`, no local player, one peer and one player per phantom.
    fn playerless(map: u32, phantoms: u32) -> coop_probe::Session {
        coop_probe::Session {
            addr: 0x8100_0000,
            state: 5,
            peers: 1 + phantoms,
            players: phantoms,
            class: 1,
            max_peers: 16,
            max_players: phantoms,
            campaign: u32::MAX,
            map,
            protocol: 2,
            difficulty: coop_probe::NO_DIFFICULTY,
            ..Default::default()
        }
    }

    #[test]
    fn the_setup_stages_follow_the_lobby() {
        let at_menu = Facts { main_menu: true, ..Default::default() };
        assert_eq!(Facts::default().setup_stage(), None);
        assert_eq!(at_menu.setup_stage(), Some(SetupStage::Menu));
        let ready = Facts { main_menu: true, networking: true, lobby_on: true, ..Default::default() };
        assert_eq!(ready.setup_stage(), Some(SetupStage::Lobby));
        assert_eq!(result_stage(Step::SquadCreated, None), None);
        assert_eq!(result_stage(Step::Start, Some("1 of 2 players")), None);
        assert_eq!(
            result_stage(Step::Start, Some("peer 1 has map status 2 (every peer needs 3 or 4)")),
            Some(SetupStage::CopyWait)
        );
        assert_eq!(result_stage(Step::Loading, None), Some(SetupStage::Load));
    }

    #[test]
    fn the_start_gate_takes_zero_local_seats_and_one_peer_per_phantom() {
        const MAP: u32 = 50;
        for phantoms in [1, 4, 15] {
            let s = playerless(MAP, phantoms);
            let n = phantoms as usize;
            assert_eq!(
                start_gate(Some(&s), Some(1), MAP, n, 1 + n),
                None,
                "{phantoms}"
            );
        }
        let s = playerless(MAP, 4);
        let gate = |s: coop_probe::Session| start_gate(Some(&s), Some(1), MAP, 4, 5);
        assert_eq!(
            start_gate(None, Some(1), MAP, 4, 5),
            Some("could not read the hosted squad".into())
        );
        assert_eq!(
            start_gate(Some(&s), Some(0), MAP, 4, 5),
            Some("session type 0 (want pregame 1 or 2)".into())
        );
        assert_eq!(
            gate(coop_probe::Session { state: 4, ..s }),
            Some("squad state 4 (want 5)".into())
        );
        assert_eq!(
            gate(coop_probe::Session { host_peer: 1, ..s }),
            Some("this machine does not host the squad".into())
        );
        assert!(
            gate(coop_probe::Session { protocol: 1, ..s })
                .is_some_and(|w| w.starts_with("protocol 1"))
        );
        assert_eq!(
            gate(coop_probe::Session { map: 51, ..s }),
            Some("map 51 (want 50)".into())
        );
        assert_eq!(
            gate(coop_probe::Session { players: 3, ..s }),
            Some("3 of 4 players".into())
        );
        // A phantom's peer without its player yet, and a peer short.
        assert!(
            gate(coop_probe::Session { peers: 4, ..s })
                .is_some_and(|w| w.starts_with("4 of 5 peers"))
        );
        // A mixed roster: two local seats on this host, two phantoms.
        let mixed = coop_probe::Session { peers: 3, ..s };
        assert_eq!(start_gate(Some(&mixed), Some(2), MAP, 4, 3), None);
    }

    #[test]
    fn startup_status_tracks_acknowledgements_and_clears_the_previous_wait() {
        let menu = || Facts {
            main_menu: true,
            networking: true,
            lobby_on: true,
            squad: Some(1234),
            squad_state: Some(0),
            session_type: Some(0),
            network_class: Some(0),
        };
        let hosted = || Facts {
            squad_state: Some(5),
            session_type: Some(1),
            network_class: Some(1),
            ..menu()
        };
        let mut state = LobbyState::default();
        assert_eq!(state.status()["squad_state"], serde_json::Value::Null);
        assert_eq!(Facts::default().wait(), Some("waiting for the main menu"));
        assert_eq!(
            Facts {
                networking: false,
                ..menu()
            }
            .wait(),
            Some("waiting for networking")
        );
        assert_eq!(
            Facts {
                lobby_on: false,
                ..menu()
            }
            .wait(),
            Some("waiting for the lobby subsystem")
        );
        assert_eq!(menu().wait(), None);
        for (progress, expected, why) in [
            (
                SimOpen::Wait("session allocation"),
                Step::Create,
                Some("session allocation"),
            ),
            (SimOpen::SquadCreated, Step::SquadCreated, None),
            (
                SimOpen::Wait("pregame lifecycle"),
                Step::SquadCreated,
                Some("pregame lifecycle"),
            ),
            (SimOpen::SessionStarted, Step::SessionStarted, None),
            (
                SimOpen::Wait("replicated parameters"),
                Step::SessionStarted,
                Some("replicated parameters"),
            ),
            (SimOpen::Ready, Step::Start, None),
        ] {
            let (step, wait) = open_step(state.step, progress);
            state.update(
                step,
                wait,
                if step == Step::Create {
                    menu()
                } else {
                    hosted()
                },
            );
            let status = state.status();
            assert_eq!(status["step"], format!("{expected:?}"));
            assert_eq!(status["wait"].as_str(), why);
            assert_eq!(
                status["session_type"],
                if step == Step::Create { 0 } else { 1 }
            );
            assert_eq!(
                status["squad_state"],
                if step == Step::Create { 0 } else { 5 }
            );
        }
        state.update(Step::Start, Some("3 of 4 players".into()), hosted());
        state.update(Step::Loading, None, hosted());
        assert!(state.setup_complete());
        assert_eq!(state.status()["wait"], serde_json::Value::Null);
        assert_eq!(state.status()["step"], "Loading");
    }

    #[test]
    fn a_retired_squad_restarts_only_an_open_lobby() {
        let active = Facts {
            squad: Some(1234),
            squad_state: Some(5),
            network_class: Some(1),
            ..Facts::default()
        };
        let freed = Facts {
            squad: Some(0),
            ..active
        };
        let cleared = Facts {
            squad_state: Some(0),
            ..active
        };
        let offline = Facts {
            network_class: Some(0),
            ..active
        };
        for step in [Step::SquadCreated, Step::SessionStarted, Step::Start] {
            assert!(!Facts::default().lost_after(step, None));
            assert!(!active.lost_after(step, Some(true)));
            assert!(!active.lost_after(step, None));
            assert!(active.lost_after(step, Some(false)));
            assert!(freed.lost_after(step, None));
            assert!(cleared.lost_after(step, None));
            assert!(offline.lost_after(step, None));
        }
        assert!(!cleared.lost_after(Step::Create, Some(false)));
        assert!(!cleared.lost_after(Step::Loading, Some(false)));
    }
}
