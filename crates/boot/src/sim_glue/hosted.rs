//! A headless host follows the System Link browser's Create Game path.
//! The playerless host waits for the registered phantom peers before starting.
use super::{Inner, Phase};
use crate::{coop_probe, level_boot};
use anyhow::{Result, bail};
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
    // Every bot is a phantom peer, so the host console plays no local
    // player of its own: its lobby is playerless and no local profile is
    // offered.
    crate::host_lobby::exclude_local_profiles(b);
    let stage = Arc::new(Mutex::new(LobbyState {
        step: Step::Create,
        in_tick: false,
        playerless: Some(crate::host_lobby::PlayerlessLobby::default()),
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
                            || s.peers != 1 + sim.matched.bots.len() as u32)
                    {
                        return Ok(Flow::Continue);
                    }
                }
            }
            let step = state.step;
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
                    let next = advance(&cx, &sim, step, map, buffer, &mut lobby).await?;
                    if next != step {
                        eprintln!("sim: System Link host {step:?} -> {next:?}");
                    }
                    // Reenter the tick with the guard set, so the real tick and other
                    // features still run exactly once.
                    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await?;
                    *stage.lock().unwrap_or_else(|e| e.into_inner()) = LobbyState {
                        step: next,
                        in_tick: false,
                        playerless: Some(lobby),
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
    lobby: &mut crate::host_lobby::PlayerlessLobby,
) -> Result<Step, EntryError> {
    match step {
        Step::Create => {
            let open = lobby
                .open_sim(cx, map, &sim.matched, buffer)
                .await
                .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
            Ok(if open { Step::Start } else { step })
        }
        Step::Start => {
            let started = lobby
                .start_sim(cx, sim.matched.bots.len())
                .await
                .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
            Ok(if started { Step::Loading } else { step })
        }
        Step::Loading => Ok(step),
    }
}
