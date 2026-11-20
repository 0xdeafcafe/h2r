//! A headless host follows the System Link browser's Create Game path.
//! The registered local pads and phantom peers join by their normal routes.
use super::{Inner, Phase};
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
                    let Some(s) = coop_probe::read_session(&*g) else {
                        return Ok(Flow::Continue);
                    };
                    if s.state != 5 || !s.is_host() {
                        return Ok(Flow::Continue);
                    }
                    if state.step == Step::Start
                        && (s.protocol != 2
                            || s.map != map
                            || s.players != sim.matched.bots.len() as u32
                            || s.peers
                                != 1 + sim.matched.bots.iter().filter(|b| b.port.is_none()).count()
                                    as u32)
                    {
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
                    let next = advance(&cx, &sim, step, map, buffer, lobby.as_mut()).await?;
                    if next != step {
                        eprintln!("sim: System Link host {step:?} -> {next:?}");
                    }
                    // Reenter the tick with the guard set, so the real tick and other
                    // features still run exactly once.
                    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await?;
                    *stage.lock().unwrap_or_else(|e| e.into_inner()) = LobbyState {
                        step: next,
                        in_tick: false,
                        playerless: lobby,
                    };
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
) -> Result<Step, EntryError> {
    match step {
        Step::Create => {
            if let Some(lobby) = playerless {
                let open = lobby
                    .open_sim(cx, map, &sim.matched, buffer)
                    .await
                    .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
                return Ok(if open { Step::Start } else { step });
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
                return Ok(Step::Configure);
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
                return Ok(step);
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
                return Ok(Step::Start);
            }
        }
        Step::Start => {
            if let Some(lobby) = playerless {
                let started = lobby
                    .start_sim(cx, sim.matched.bots.len())
                    .await
                    .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
                return Ok(if started { Step::Loading } else { step });
            }
            if sim.matched.team_play {
                cx.write(|m| -> Result<(), engine::MemFault> {
                    use bots::phantom::session::{self, at::*};
                    let s = m.u32(points::SQUAD_SESSION.base)?;
                    let local = m.u32(s + LOCAL_PEER)?;
                    let mask = m.u32(s + PLAYER_MASK)?;
                    for slot in (0..session::MAX).filter(|i| mask & (1 << i) != 0) {
                        let p = s + PLAYERS + slot * PLAYER_SIZE;
                        if m.u32(p + P_PEER)? != local {
                            continue;
                        }
                        let controller = m.u32(p + P_CONTROLLER)?;
                        let Some(team) = sim
                            .matched
                            .bots
                            .iter()
                            .find(|b| b.port.map(u32::from) == Some(controller))
                            .and_then(|b| b.team)
                        else {
                            continue;
                        };
                        if m.u8(p + P_EFFECTIVE + CONFIG_TEAM as u32)? != team
                            || m.u8(p + P_REQUESTED + CONFIG_TEAM as u32)? != team
                        {
                            session::set_team(m, s, slot, team)?;
                        }
                    }
                    Ok(())
                })?;
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
                return Ok(Step::Loading);
            }
        }
        Step::Loading => (),
    }
    Ok(step)
}
