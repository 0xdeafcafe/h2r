//! The sim backend for phantom peers. Boot implements it over
//! `bots::phantom`, which seats a remote player through the game's
//! membership. A peer is driven by a script, recording or brain.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_proto::sim::{BotBrain, ClankerView, Identity};
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
pub struct Bot {
    /// The backend's handle for the phantom peer.
    pub handle: u32,
}

/// The bots, as sim calls them.
pub trait Bots: Send {
    /// `bots::phantom::add`: a remote player with a server-issued
    /// identity. A handle for it.
    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        source: PadSource,
    ) -> Result<u32, String>;
    /// Remove the phantom peer.
    fn remove(&mut self, bot: Bot) -> Result<(), String>;
    /// What a brain-driven simulated player is thinking.
    fn brain_view(&self, _bot: Bot) -> Option<BotBrain> {
        None
    }
    /// All brain-driven bots on the host.
    fn clankers(&self, _guest: Option<&dyn guest_layout::GuestMemory>) -> Vec<ClankerView> {
        Vec::new()
    }

    /// The players-array index of the player a phantom became, once it has
    /// one.
    fn phantom_player(&self, handle: u32) -> Option<u32> {
        let _ = handle;
        None
    }

    /// How many phantom peers have joined the session now. Each is a machine
    /// with one player in the session's peer count, so the peers that are
    /// people are the session's peers less the host and these. A backend that
    /// cannot say reports none, which counts every peer as a person.
    fn joined_phantoms(&self) -> u32 {
        0
    }
}

/// A game without a bots backend.
pub struct NotLanded;

impl Bots for NotLanded {
    fn add_phantom(&mut self, _: &Identity, _: Option<i32>, _: PadSource) -> Result<u32, String> {
        Err("phantom sim players need the bots::phantom backend, unavailable here".into())
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
    pub brains: BTreeMap<Bot, BotBrain>,
    pub clankers: Vec<ClankerView>,
}

/// A backend for tests with phantoms numbered from 1. Clones share one log.
#[derive(Clone, Default)]
pub struct FakeBots(pub Arc<Mutex<FakeLog>>);

impl FakeBots {
    pub fn log(&self) -> std::sync::MutexGuard<'_, FakeLog> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Bots for FakeBots {
    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        source: PadSource,
    ) -> Result<u32, String> {
        let mut l = self.log();
        let handle = l.phantoms.len() as u32 + 1;
        l.phantoms.insert(handle, (identity.clone(), team));
        l.bots.insert(Bot { handle }, source);
        Ok(handle)
    }

    fn remove(&mut self, bot: Bot) -> Result<(), String> {
        let mut l = self.log();
        l.bots.remove(&bot).ok_or("no such bot")?;
        l.brains.remove(&bot);
        l.removed.push(bot);
        Ok(())
    }

    fn brain_view(&self, bot: Bot) -> Option<BotBrain> {
        self.log().brains.get(&bot).cloned()
    }

    fn clankers(&self, _guest: Option<&dyn guest_layout::GuestMemory>) -> Vec<ClankerView> {
        self.log().clankers.clone()
    }

    fn phantom_player(&self, handle: u32) -> Option<u32> {
        self.log().phantom_players.get(&handle).copied()
    }

    fn joined_phantoms(&self) -> u32 {
        self.log().phantom_players.len() as u32
    }
}
