//! A headless host follows the System Link browser's Create Game path.
//! The registered local pads and phantom peers join by their normal routes.
use super::{Inner, Phase};
use crate::host_lobby::SimStart;
use crate::{coop_probe, level_boot};
use anyhow::{Result, bail};
use bots::guest;
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use feature::{Flow, points};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};

/// Hold the eighth port of a block for the lifetime of this process. Retail
/// uses base through base+6, and discovery stays on this host's own endpoint.
pub(crate) struct Network {
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
    Configure,
    Start,
    Loading,
}

struct LobbyState {
    step: Step,
    in_tick: bool,
    playerless: Option<crate::host_lobby::PlayerlessLobby>,
    /// The last reason logged for a wait, so each change is said once.
    waiting: Option<String>,
}

/// What the host's squad must show before `Step::Start` asks to start:
/// live (state 5), hosted here, distributed, on the match's map, with one
/// player per seat and one peer per machine (this host's own and each
/// phantom's). `None`: ready to try. The same for a playerless host and a
/// mixed roster.
fn start_gate(
    s: Option<&coop_probe::Session>,
    map: u32,
    seats: usize,
    machines: usize,
) -> Option<String> {
    let Some(s) = s else {
        return Some("no squad session".into());
    };
    if s.state != 5 {
        return Some(format!("squad state {} (want 5)", s.state));
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

/// Says `why` once per change (and once that the wait is over).
fn note_wait(state: &mut LobbyState, step: Step, why: Option<String>) {
    if state.waiting != why {
        match &why {
            Some(w) => eprintln!("sim: System Link host waits at {step:?}: {w}"),
            None if state.waiting.is_some() => {
                eprintln!("sim: System Link host {step:?}: no longer waiting")
            }
            None => (),
        }
        state.waiting = why;
    }
}

pub(super) fn install(
    b: &mut feature::Builder,
    sim: Arc<Inner>,
) -> Result<(), feature::BuildError> {
    let buffer = b.guest_alloc(0x1000, runtime::guest::Permissions::RW)?;
    let playerless = sim.matched.playerless_host();
    if playerless {
        crate::host_lobby::exclude_local_profiles(b);
    }
    let lobby = playerless.then(crate::host_lobby::PlayerlessLobby::default);
    let stage = Arc::new(Mutex::new(LobbyState {
        step: Step::Create,
        in_tick: false,
        playerless: lobby,
        waiting: None,
    }));
    b.raw()
        .before(&points::GAME_TICK, "sim: System Link host", move |c| {
            if sim.lock().phase != Phase::Waiting {
                return Ok(Flow::Continue);
            }
            let Some(req) = sim.hosted_map.get() else {
                return Ok(Flow::Continue);
            };
            let map = req.map;
            let mut state = stage.lock().unwrap_or_else(|e| e.into_inner());
            if state.in_tick || state.step == Step::Loading {
                return Ok(Flow::Continue);
            }
            {
                let g = c.ctx.runtime().memory();
                if level_boot::active_mode(&*g) != Some(level_boot::MODE_MAINMENU)
                    || !g.u8(points::NETWORK_READY.base).is_ok_and(|r| r != 0)
                    || !g.u32(points::SQUAD_SESSION.base).is_ok_and(|p| p != 0)
                    || !g.u8(bots::lobby::LOBBY_ON).is_ok_and(|r| r != 0)
                {
                    return Ok(Flow::Continue);
                }
                if state.step != Step::Create {
                    let s = coop_probe::read_session(&*g);
                    let why = if state.step == Step::Start {
                        let phantoms = sim.matched.bots.iter().filter(|b| b.port.is_none());
                        start_gate(s.as_ref(), map, sim.matched.bots.len(), 1 + phantoms.count())
                    } else {
                        match s {
                            None => Some("no squad session".into()),
                            Some(s) if s.state != 5 || !s.is_host() => Some(format!(
                                "squad state {}, hosted here {}",
                                s.state,
                                s.is_host()
                            )),
                            Some(_) => None,
                        }
                    };
                    if why.is_some() {
                        let step = state.step;
                        note_wait(&mut state, step, why);
                        return Ok(Flow::Continue);
                    }
                }
            }
            let step = state.step;
            let mut lobby = state.playerless.take();
            state.in_tick = true;
            drop(state);
            let sim = sim.clone();
            let stage = stage.clone();
            c.answer_async(move |cx| {
                Box::pin(async move {
                    let (next, why) =
                        advance(&cx, &sim, step, map, buffer, lobby.as_mut()).await?;
                    if next != step {
                        eprintln!("sim: System Link host {step:?} -> {next:?}");
                    }
                    // Reenter the tick with the guard set, so the real tick and other
                    // features still run exactly once.
                    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await?;
                    {
                        let mut state = stage.lock().unwrap_or_else(|e| e.into_inner());
                        state.step = next;
                        state.in_tick = false;
                        state.playerless = lobby;
                        note_wait(&mut state, next, if next == step { why } else { None });
                    }
                    Ok(Ret::new(r.eax, 0))
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
    playerless: Option<&mut crate::host_lobby::PlayerlessLobby>,
) -> Result<(Step, Option<String>), EntryError> {
    match step {
        Step::Create => {
            if let Some(lobby) = playerless {
                let open = lobby
                    .open_sim(cx, map, &sim.matched, buffer)
                    .await
                    .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
                return Ok(if open {
                    (Step::Start, None)
                } else {
                    (step, Some("opening the playerless lobby".into()))
                });
            }
            for bot in &sim.matched.bots {
                let Some(port) = bot.port.map(u32::from) else {
                    continue;
                };
                if !cx.read(|m| guest::signed_in(m, port))? {
                    let block = guest::bot_block(&[0; guest::BLOCK_LEN], &bot.name);
                    cx.write(|m| m.write(buffer, &block))?;
                    cx.call(GuestCall::new(points::PROFILE_SIGN_IN.base).args([
                        port,
                        buffer,
                        guest::NO_FILE,
                    ]))
                    .await?;
                    cx.write(|m| guest::set_slot_name(m, port, &bot.name))?;
                }
            }
            // LAN Create New Game row, 0x253116..0x253131.
            if cx.call(GuestCall::new(0x199df9).args([0, 0])).await?.eax as u8 != 0 {
                cx.call(GuestCall::new(0x199a03).args([2])).await?;
                return Ok((Step::Configure, None));
            }
        }
        Step::Configure => {
            if cx
                .call(
                    GuestCall::new(points::LOBBY_SET_MAP.base)
                        .esi(map)
                        .args([u32::MAX]),
                )
                .await?
                .eax as u8
                == 0
            {
                return Ok((step, Some("retail refused the map".into())));
            }
            cx.call(
                GuestCall::new(level_boot::VARIANT_DEFAULTS)
                    .args([buffer, sim.matched.game_type.list_index()]),
            )
            .await?;
            let mut raw = [0; level_boot::VARIANT_SIZE as usize];
            cx.read(|m| m.read(buffer, &mut raw))?;
            let v = level_boot::configured_variant(&raw, &sim.matched.variant)
                .map_err(EntryError::IsolatedCall)?;
            cx.write(|m| m.write(buffer, &v.to_bytes()))?;
            if cx.call(GuestCall::new(0x19a728).args([buffer])).await?.eax as u8 != 0 {
                return Ok((Step::Start, None));
            }
        }
        Step::Start => {
            if let Some(lobby) = playerless {
                let started = lobby
                    .start_sim(cx, sim.matched.bots.len())
                    .await
                    .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
                return Ok(match started {
                    SimStart::Started => (Step::Loading, None),
                    SimStart::Wait(why) => (step, Some(why)),
                });
            }
            if sim.matched.team_play {
                let mut ready = true;
                for bot in &sim.matched.bots {
                    if let (Some(port), Some(team)) = (bot.port, bot.team) {
                        ready &= local_team_ready(cx, u32::from(port), team).await?;
                    }
                }
                if !ready {
                    return Ok((step, Some("local seats' teams".into())));
                }
            }
            // Same countdown and controller as the lobby's Start Game row.
            if cx
                .call(GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
                .await?
                .eax
                == 9
                && cx
                    .call(GuestCall::new(0x19a78e).eax(0).args([10, 3]))
                    .await?
                    .eax as u8
                    != 0
            {
                return Ok((Step::Loading, None));
            }
        }
        Step::Loading => (),
    }
    Ok((step, None))
}

// The lobby team picker (0x250d4d) changes the controller's preference and
// calls 0x18fe9e. Every later local property refresh reads the same dword
// at 0x18fe74. Editing just the session leaves that source stale.
const SLOT_TEAM: u32 = 0x1fc;
const PUBLISH_PROPERTIES: u32 = 0x18fe9e;

async fn local_team_ready(cx: &Cx, port: u32, team: u8) -> Result<bool, EntryError> {
    use bots::phantom::session::{self, at::*};

    let profile = guest::slot(port);
    let user = cx.read(|m| m.u32(profile + guest::SLOT_LOCAL_USER))?;
    if user >= 4 || !cx.read(|m| guest::signed_in(m, port))? {
        return Ok(false);
    }
    if cx.read(|m| m.u32(profile + SLOT_TEAM))? != u32::from(team) {
        cx.write(|m| m.set_u32(profile + SLOT_TEAM, u32::from(team)))?;
        cx.call(GuestCall::new(PUBLISH_PROPERTIES).edi(port))
            .await?;
        return Ok(false);
    }

    // 0x65cb0 can retain an existing effective team. Keep the session
    // copies in sync and check them again after the next retail tick.
    Ok(cx.write(|m| -> Result<bool, engine::MemFault> {
        let s = m.u32(points::SQUAD_SESSION.base)?;
        let local = m.u32(s + LOCAL_PEER)?;
        let mask = m.u32(s + PLAYER_MASK)?;
        for slot in (0..session::MAX).filter(|i| mask & (1 << i) != 0) {
            let p = s + PLAYERS + slot * PLAYER_SIZE;
            if m.u32(p + P_PEER)? == local && m.u32(p + P_CONTROLLER)? == port {
                if m.u8(p + P_REQUESTED + CONFIG_TEAM as u32)? == team
                    && m.u8(p + P_EFFECTIVE + CONFIG_TEAM as u32)? == team
                {
                    return Ok(true);
                }
                session::set_team(m, s, slot, team)?;
                break;
            }
        }
        Ok(false)
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bots::phantom::session::at::*;
    use engine::entry::{Regs, testing::run};
    use engine::memory::GuestMemory;
    use engine::testing::FakeGuest;

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
    fn the_start_gate_takes_zero_local_seats_and_one_peer_per_phantom() {
        const MAP: u32 = 50;
        for phantoms in [1, 4, 15] {
            let s = playerless(MAP, phantoms);
            let n = phantoms as usize;
            assert_eq!(start_gate(Some(&s), MAP, n, 1 + n), None, "{phantoms}");
        }
        let s = playerless(MAP, 4);
        let gate = |s: coop_probe::Session| start_gate(Some(&s), MAP, 4, 5);
        assert_eq!(start_gate(None, MAP, 4, 5), Some("no squad session".into()));
        assert_eq!(
            gate(coop_probe::Session { state: 4, ..s }),
            Some("squad state 4 (want 5)".into())
        );
        assert_eq!(
            gate(coop_probe::Session { host_peer: 1, ..s }),
            Some("this machine does not host the squad".into())
        );
        assert!(gate(coop_probe::Session { protocol: 1, ..s })
            .is_some_and(|w| w.starts_with("protocol 1")));
        assert_eq!(
            gate(coop_probe::Session { map: 51, ..s }),
            Some("map 51 (want 50)".into())
        );
        assert_eq!(
            gate(coop_probe::Session { players: 3, ..s }),
            Some("3 of 4 players".into())
        );
        // A phantom's peer without its player yet, and a peer short.
        assert!(gate(coop_probe::Session { peers: 4, ..s })
            .is_some_and(|w| w.starts_with("4 of 5 peers")));
        // A mixed roster: two local seats on this host, two phantoms.
        let mixed = coop_probe::Session { peers: 3, ..s };
        assert_eq!(start_gate(Some(&mixed), MAP, 4, 3), None);
    }

    #[test]
    fn a_wait_is_said_once_per_reason() {
        let mut state = LobbyState {
            step: Step::Start,
            in_tick: false,
            playerless: None,
            waiting: None,
        };
        note_wait(&mut state, Step::Start, Some("a".into()));
        assert_eq!(state.waiting.as_deref(), Some("a"));
        note_wait(&mut state, Step::Start, Some("a".into()));
        note_wait(&mut state, Step::Start, Some("b".into()));
        assert_eq!(state.waiting.as_deref(), Some("b"));
        note_wait(&mut state, Step::Loading, None);
        assert_eq!(state.waiting, None);
    }

    #[test]
    fn blue_local_publishes_its_controller_preference_before_countdown() {
        const S: u32 = 0x800000;
        const PORT: u32 = 1;
        let profile = guest::slot(PORT);
        let remote = S + PLAYERS;
        let local = remote + PLAYER_SIZE;
        let mut g = FakeGuest::new();
        g.put_u32(points::SQUAD_SESSION.base, S)
            .put_u32(S + LOCAL_PEER, 2)
            .put_u32(S + PLAYER_MASK, 3)
            .put_u32(S + UPDATE, 5)
            .put_u32(S + CHANGES, 8)
            .put_u32(profile, guest::SLOT_PROFILE)
            .put_u32(profile + guest::SLOT_LOCAL_USER, 0)
            .put_u32(profile + SLOT_TEAM, 0)
            .put_u32(remote + P_PEER, 1)
            .put_u32(remote + P_CONTROLLER, PORT)
            .put_u8(remote + P_REQUESTED + CONFIG_TEAM as u32, 1)
            .put_u8(remote + P_EFFECTIVE + CONFIG_TEAM as u32, 1)
            .put_u32(local + P_PEER, 2)
            .put_u32(local + P_CONTROLLER, PORT)
            .put_u8(local + P_EFFECTIVE + CONFIG_TEAM as u32, 0);
        let tick = |g| {
            run(
                &|cx| {
                    Box::pin(async move {
                        Ok(Ret::new(
                            u32::from(local_team_ready(&cx, PORT, 1).await?),
                            0,
                        ))
                    })
                },
                g,
                Regs::default(),
                |call, m| {
                    assert_eq!(*call, GuestCall::new(PUBLISH_PROPERTIES).edi(PORT));
                    assert_eq!(m.u32(profile + SLOT_TEAM), Ok(1));
                    Regs::default()
                },
            )
        };
        let first = tick(g);
        assert_eq!(first.calls.len(), 1);
        assert_eq!(first.result, Ok(Ret::new(0, 0)));
        let mut g = first.guest;
        g.put_u8(local + P_REQUESTED + CONFIG_TEAM as u32, 1);
        let pending = tick(g);
        assert!(pending.calls.is_empty());
        assert_eq!(pending.result, Ok(Ret::new(0, 0)));
        assert_eq!(
            pending.guest.u8(local + P_EFFECTIVE + CONFIG_TEAM as u32),
            Ok(1)
        );
        assert_eq!(pending.guest.u32(S + UPDATE), Ok(6));
        let ready = tick(pending.guest);
        assert!(ready.calls.is_empty());
        assert_eq!(ready.result, Ok(Ret::new(1, 0)));
    }
}
