//! `sim.player.*`'s bots (`agent::sim::bots::Bots`) over docs/game/bots.md's
//! `bots::local` and `bots::phantom`.
//!
//! A local sim player is a Clanker on a free port after the human's, in the
//! process-wide [`bots::local::global`] the boot glue drives, so it signs in
//! and plays like an `H2_BOTS` one. Its source:
//!
//! - a script is the `Remote` sim fills (`sim.player.add`'s steps, then
//!   `sim.player.pad`), read each frame with `state_at_tick` then
//!   `state_read`, as the agent's input tap reads `input.pad`'s. The bots
//!   are asked once a rendered frame, so a step's `ticks` count the bot
//!   frames ([`Frame::frame`]), not game ticks; `ms` is wall time either
//!   way;
//! - a recording is an input log's reads of its first pad, one per frame
//!   ([`bots::local::Recorded`]);
//! - the brain is [`bots::Brain`] at `H2_BOTS_SKILL`.
//!
//! Until a bot is signed in on the players screen, every source presses A
//! to join ([`bots::local::join_pad`]), as the brain does.
//!
//! Phantoms go to `bots::phantom`: on the books at once, they join once a
//! multiplayer game this console hosts runs (docs/game/bots.md §5.1).

use std::collections::BTreeMap;
use std::time::Instant;

use agent::sim::bots::{Bot, Bots, PadSource};
use agent_proto::sim::Identity;
use bots::local::{self, BotHandle, Frame, Local};
use input::backend::remote::Remote;
use input::gamepad::{Gamepad, State};
use input::host::HostPadState;

/// The bots of one [`Local`], as sim calls them.
pub struct LocalBots {
    local: Local,
    /// The bot on each port sim asked for.
    handles: BTreeMap<u32, BotHandle>,
}

impl LocalBots {
    pub fn new(local: Local) -> Self {
        LocalBots {
            local,
            handles: BTreeMap::new(),
        }
    }
}

/// A script source: the pad `Remote` holds now, timed in bot frames.
struct ScriptPad(Remote);

impl local::PadSource for ScriptPad {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        if f.joining {
            return local::join_pad(f.frame);
        }
        let g = self.0.state_at_tick(Instant::now(), Some(f.frame));
        self.0.state_read();
        HostPadState::from_gamepad(g)
    }
}

/// Signs in first, then plays `inner` from the first frame after.
struct Joining<S>(S);

impl<S: local::PadSource> local::PadSource for Joining<S> {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        if f.joining {
            local::join_pad(f.frame)
        } else {
            self.0.pad(f)
        }
    }
}

/// An input log's pads: the successful reads of the first handle it read.
pub fn recording_pads(text: &str) -> Result<Vec<HostPadState>, String> {
    let log = xapi::tap::InputLog::parse(text)?;
    let Some(first) = log.reads.first().map(|r| r.handle) else {
        return Err("an empty input log".into());
    };
    Ok(log
        .reads
        .iter()
        .filter(|r| r.handle == first && r.status == 0)
        .map(|r| {
            let pad: &[u8; Gamepad::SIZE] = r.packet[State::SIZE - Gamepad::SIZE..]
                .try_into()
                .expect("a packet holds a gamepad");
            HostPadState::from_gamepad(Gamepad::from_bytes(pad))
        })
        .collect())
}

fn source(s: PadSource, port: u8) -> Result<Box<dyn local::PadSource>, String> {
    Ok(match s {
        PadSource::Script(r) => Box::new(ScriptPad(r)),
        PadSource::Recording { name, path } => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("recording {name} ({}): {e}", path.display()))?;
            let pads = recording_pads(&text).map_err(|e| format!("recording {name}: {e}"))?;
            Box::new(Joining(local::Recorded::new(pads)))
        }
        PadSource::Brain => {
            let skill = std::env::var(bots::SKILL_ENV)
                .ok()
                .and_then(|v| bots::mind::Skill::parse(&v))
                .unwrap_or_default();
            Box::new(bots::Brain::new(skill, 0x5eed ^ u64::from(port)))
        }
    })
}

/// An xuid as the server gives it: hex, `0x` optional.
fn xuid(s: &str) -> Result<u64, String> {
    let t = s.trim();
    u64::from_str_radix(t.strip_prefix("0x").unwrap_or(t), 16)
        .map_err(|_| format!("xuid {s:?}: hex, as the local embedded server issues it"))
}

impl Bots for LocalBots {
    fn add_local(&mut self, port: Option<u32>, src: PadSource) -> Result<u32, String> {
        let wanted: Vec<u8> = self.local.list().iter().map(|b| b.wanted).collect();
        let port = match port {
            Some(p) => u8::try_from(p).map_err(|_| format!("port {p}: ports are 0 to 3"))?,
            None => (1..=3)
                .find(|p| !wanted.contains(p))
                .ok_or_else(|| format!("{} local bots already", local::MAX_LOCAL))?,
        };
        let src = source(src, port)?;
        let h = self
            .local
            .add(port, &format!("Clanker {port}"), src)
            .map_err(|e| e.to_string())?;
        self.handles.insert(u32::from(port), h);
        Ok(u32::from(port))
    }

    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        src: PadSource,
    ) -> Result<u32, String> {
        let id = bots::phantom::Identity {
            xuid: xuid(&identity.xuid)?,
            gamertag: identity.gamertag.clone(),
            ..Default::default()
        };
        let team = team
            .map(|t| u8::try_from(t).map_err(|_| format!("team {t}")))
            .transpose()?;
        // A phantom has no port; the brain's seed only needs to differ.
        let src = source(src, 4)?;
        bots::phantom::add(id, team, src)
            .map(|h| h.0)
            .map_err(|e| e.to_string())
    }

    fn placed_port(&self, port: u32) -> Option<u32> {
        let h = self.handles.get(&port)?;
        self.local
            .list()
            .into_iter()
            .find(|b| b.handle == *h)?
            .port
            .map(u32::from)
    }

    fn phantom_player(&self, handle: u32) -> Option<u32> {
        bots::phantom::player(bots::phantom::PhantomHandle(handle))
    }

    fn remove(&mut self, bot: Bot) -> Result<(), String> {
        match bot {
            Bot::Local { port } => {
                let h = self
                    .handles
                    .remove(&port)
                    .ok_or_else(|| format!("no sim bot on port {port}"))?;
                self.local.remove(h).map_err(|e| e.to_string())
            }
            Bot::Phantom { handle } => bots::phantom::remove(bots::phantom::PhantomHandle(handle))
                .map_err(|e| e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::backend::remote::Step;
    use input::host::HostButton;

    fn frame<'a>(g: &'a engine::testing::FakeGuest, n: u64, joining: bool) -> Frame<'a> {
        Frame {
            frame: n,
            tick: None,
            port: 1,
            machine: None,
            guest: g,
            joining,
        }
    }

    #[test]
    fn sim_players_are_clankers_on_the_free_ports() {
        let local = Local::default();
        local
            .add(1, "Clanker 1", Box::new(bots::Brain::default()))
            .unwrap();
        let mut b = LocalBots::new(local.clone());
        assert_eq!(
            b.add_local(None, PadSource::Script(Remote::new())),
            Ok(2),
            "port 1 has an H2_BOTS bot"
        );
        assert_eq!(b.add_local(None, PadSource::Brain), Ok(3));
        assert!(b.add_local(None, PadSource::Brain).is_err(), "full");
        assert!(b.add_local(Some(2), PadSource::Brain).is_err(), "taken");
        assert_eq!(local.list().len(), 3);
        assert_eq!(
            b.placed_port(2),
            None,
            "not placed until the input manager syncs"
        );
        b.remove(Bot::Local { port: 2 }).unwrap();
        assert_eq!(local.list().len(), 2);
        assert!(
            b.remove(Bot::Local { port: 1 }).is_err(),
            "sim removes only its own"
        );
        // A phantom goes on the process-wide books, pending until a hosted
        // game runs (docs/game/bots.md §5.1).
        let h = b
            .add_phantom(
                &Identity {
                    xuid: "000e0000c0ffee01".into(),
                    gamertag: "Clanker".into(),
                },
                Some(1),
                PadSource::Brain,
            )
            .unwrap();
        assert_eq!(b.phantom_player(h), None, "no player outside a game");
        b.remove(Bot::Phantom { handle: h }).unwrap();
        assert!(
            b.add_phantom(
                &Identity {
                    xuid: "zz".into(),
                    gamertag: "x".into()
                },
                None,
                PadSource::Brain
            )
            .unwrap_err()
            .contains("hex")
        );
    }

    #[test]
    fn a_script_joins_then_plays_its_steps() {
        let g = engine::testing::FakeGuest::new();
        let r = Remote::new();
        let mut pad = Gamepad::default();
        pad.buttons = input::gamepad::Buttons::START;
        r.replace([Step {
            pad,
            ..Default::default()
        }]);
        let mut s = ScriptPad(r.clone());
        use local::PadSource as _;
        assert!(
            s.pad(&frame(&g, 0, true))
                .buttons
                .contains(HostButton::South)
        );
        assert!(r.busy(), "joining leaves the steps queued");
        assert!(
            s.pad(&frame(&g, 1, false))
                .buttons
                .contains(HostButton::Start)
        );
    }

    #[test]
    fn a_recording_is_its_first_pads_reads() {
        let mut pad = Gamepad::default();
        pad.thumb_lx = i16::MAX;
        let st = |n, g: Gamepad| {
            State {
                packet_number: n,
                gamepad: g,
            }
            .to_bytes()
        };
        let log = xapi::tap::InputLog {
            reads: vec![
                xapi::tap::Read {
                    tick: None,
                    handle: 7,
                    status: 0,
                    packet: st(1, pad),
                },
                xapi::tap::Read {
                    tick: None,
                    handle: 8,
                    status: 0,
                    packet: st(1, Gamepad::default()),
                },
                xapi::tap::Read {
                    tick: None,
                    handle: 7,
                    status: 1167,
                    packet: st(2, pad),
                },
                xapi::tap::Read {
                    tick: None,
                    handle: 7,
                    status: 0,
                    packet: st(3, Gamepad::default()),
                },
            ],
        };
        let pads = recording_pads(&log.to_text()).unwrap();
        assert_eq!(pads.len(), 2);
        assert!(pads[0].left_stick.0 > 0.99);
        assert_eq!(pads[1], HostPadState::from_gamepad(Gamepad::default()));
        assert!(recording_pads("nope").is_err());
    }
}
