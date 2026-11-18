//! The seam to the bots (docs/game/bots.md), owned by the mp-bots lane.
//!
//! bots.md's library calls, as sim needs them:
//!
//! - `bots::local::add(port, PadSource)` and `remove` (stage 1: a pad on a
//!   free port after the human's);
//! - `bots::phantom::add(identity, team, PadSource)` and `remove` (stage 6:
//!   a remote player made on the host through the game's own membership);
//! - `PadSource`: scripted, recorded or the brain.
//!
//! This crate doesn't depend on `bots`: [`Bots`] is shaped like those
//! calls, and boot implements it over them (`boot::sim_bots`, phantoms
//! still refused until stage 6). [`NotLanded`] refuses every add, for a
//! game without bots; [`FakeBots`] is for tests.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_proto::sim::Identity;
use input::backend::remote::Remote;

/// Where a bot's pad comes from (bots.md's `PadSource`). The brain emits
/// only pad state, so a sim script is just another source.
#[derive(Debug, Clone)]
pub enum PadSource {
    /// Steps sim queues on this handle (`sim.player.add`'s `steps`, then
    /// `sim.player.pad`). The backend reads the pad each poll with
    /// `Remote::state_at_tick` and calls `state_read` after a read, as the
    /// agent's input tap does.
    Script(Remote),
    /// An input log (`xapi::tap` format), answered read for read.
    Recording { name: String, path: PathBuf },
    /// The bot brain (bots.md §3).
    Brain,
}

impl PadSource {
    pub fn kind(&self) -> agent_proto::sim::SourceKind {
        use agent_proto::sim::SourceKind;
        match self {
            PadSource::Script(_) => SourceKind::Script,
            PadSource::Recording { .. } => SourceKind::Recording,
            PadSource::Brain => SourceKind::Brain,
        }
    }
}

/// A bot the backend made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bot {
    /// A local slot: its controller port.
    Local { port: u32 },
    /// A phantom peer: the backend's handle for it.
    Phantom { handle: u32 },
}

/// The bots, as sim calls them.
pub trait Bots: Send {
    /// `bots::local::add`: a pad on `port`, or the lowest free port after
    /// the real pads. The port it took.
    fn add_local(&mut self, port: Option<u32>, source: PadSource) -> Result<u32, String>;
    /// `bots::phantom::add`: a remote player with a server-issued
    /// identity. A handle for it.
    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        source: PadSource,
    ) -> Result<u32, String>;
    /// `remove` of either kind.
    fn remove(&mut self, bot: Bot) -> Result<(), String>;
    /// The port the local bot that asked for `port` is on: the input
    /// manager moves a bot off a port a real pad holds. `None` until it is
    /// placed.
    fn placed_port(&self, port: u32) -> Option<u32> {
        Some(port)
    }
    /// The players-array index of the player a phantom became, once it has
    /// one. Local bots are found by their port (the player's local user).
    fn phantom_player(&self, handle: u32) -> Option<u32> {
        let _ = handle;
        None
    }
}

/// No bots: every add is refused, naming the bots.md stage it needs.
pub struct NotLanded;

impl Bots for NotLanded {
    fn add_local(&mut self, _: Option<u32>, _: PadSource) -> Result<u32, String> {
        Err(
            "local sim players need bots::local (docs/game/bots.md stage 1), not installed here; \
             input.pad steers a real pad's port"
                .into(),
        )
    }

    fn add_phantom(&mut self, _: &Identity, _: Option<i32>, _: PadSource) -> Result<u32, String> {
        Err(
            "phantom sim players need bots::phantom (docs/game/bots.md stage 6), not installed here"
                .into(),
        )
    }

    fn remove(&mut self, _: Bot) -> Result<(), String> {
        Err("no bots backend".into())
    }
}

/// What a [`FakeBots`] was asked, for tests to read.
#[derive(Debug, Default)]
pub struct FakeLog {
    /// Live bots and their sources.
    pub bots: BTreeMap<Bot, PadSource>,
    pub phantoms: BTreeMap<u32, (Identity, Option<i32>)>,
    pub removed: Vec<Bot>,
    /// The player index each phantom became.
    pub phantom_players: BTreeMap<u32, u32>,
}

/// A backend for tests: local ports from 1 (the human has 0) to 3,
/// phantoms numbered from 1. Clones share one log.
#[derive(Clone, Default)]
pub struct FakeBots(pub Arc<Mutex<FakeLog>>);

impl FakeBots {
    pub fn log(&self) -> std::sync::MutexGuard<'_, FakeLog> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Bots for FakeBots {
    fn add_local(&mut self, port: Option<u32>, source: PadSource) -> Result<u32, String> {
        let mut l = self.log();
        let taken = |p: u32| l.bots.contains_key(&Bot::Local { port: p });
        let port = match port {
            Some(p) if !(1..4).contains(&p) => return Err(format!("port {p}: 1 to 3")),
            Some(p) if taken(p) => return Err(format!("port {p} is taken")),
            Some(p) => p,
            None => (1..4).find(|p| !taken(*p)).ok_or("no free port")?,
        };
        l.bots.insert(Bot::Local { port }, source);
        Ok(port)
    }

    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        source: PadSource,
    ) -> Result<u32, String> {
        let mut l = self.log();
        let handle = l.phantoms.len() as u32 + 1;
        l.phantoms.insert(handle, (identity.clone(), team));
        l.bots.insert(Bot::Phantom { handle }, source);
        Ok(handle)
    }

    fn remove(&mut self, bot: Bot) -> Result<(), String> {
        let mut l = self.log();
        l.bots.remove(&bot).ok_or("no such bot")?;
        l.removed.push(bot);
        Ok(())
    }

    fn phantom_player(&self, handle: u32) -> Option<u32> {
        self.log().phantom_players.get(&handle).copied()
    }
}
