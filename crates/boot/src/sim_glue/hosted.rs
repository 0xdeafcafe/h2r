//! A headless host follows the System Link browser's Create Game path.
//! The playerless host waits for the registered phantom peers before starting.
use super::{Inner, Phase};
use crate::host_lobby::SimStart;
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
    // Every bot is a phantom peer, so the host console plays no local
    // player of its own: its lobby is playerless and no local profile is
    // offered.
    crate::host_lobby::exclude_local_profiles(b);
    let stage = Arc::new(Mutex::new(LobbyState {
        step: Step::Create,
        in_tick: false,
        playerless: Some(crate::host_lobby::PlayerlessLobby::default()),
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
                        let seats = sim.matched.bots.len();
                        start_gate(s.as_ref(), map, seats, 1 + seats)
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
                    let (next, why) =
                        advance(&cx, &sim, step, map, buffer, &mut lobby).await?;
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
                        state.playerless = Some(lobby);
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
    lobby: &mut crate::host_lobby::PlayerlessLobby,
) -> Result<(Step, Option<String>), EntryError> {
    match step {
        Step::Create => {
            let open = lobby
                .open_sim(cx, map, &sim.matched, buffer)
                .await
                .map_err(|e| EntryError::IsolatedCall(e.to_string()))?;
            Ok(if open {
                (Step::Start, None)
            } else {
                (step, Some("opening the playerless lobby".into()))
            })
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
}
