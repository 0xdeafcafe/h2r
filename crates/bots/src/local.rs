//! Local bots: pads on controller ports driven by a [`PadSource`]
//! (docs/game/bots.md, section 4 and "Library surface").
//!
//! [`add`] puts a bot on a port, [`remove`] takes it off, [`list`] says
//! what is there. Each frame the boot glue asks every bot's source for its
//! pad ([`Local::frame`]) on the game thread. The input [`Backend`] reports
//! the bots as always-connected pads, each asking for its own port
//! ([`input::HostBackend::preferred_port`]); a port a real pad already
//! holds sends the bot to the lowest free one, and [`list`] says where it
//! went. The game sees an inserted controller and reads it through
//! `XInputGetState` like any other; removing a bot unplugs it.
//!
//! The free functions work on one process-wide [`Local`] ([`global`]),
//! which the boot glue drives. Tests make their own.

use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use input::HostBackend;
use input::host::{HostButton, HostPad, HostPadId, HostPadState};

/// The most local bots: four ports, one human.
pub const MAX_LOCAL: usize = 3;

/// The first [`HostPadId`] of a bot pad; the bot with handle `h` is
/// `ID_BASE + h`.
pub const ID_BASE: u64 = 0xc1a4_0000;

/// What a source sees each frame.
pub struct Frame<'a> {
    /// Frames since the glue started: once a rendered frame, menus
    /// included.
    pub frame: u64,
    /// The game's tick (30 a second) while a game is loaded, `None` in the
    /// menus ([`crate::guest::game_tick`]). Ticks are not frames: a slow
    /// frame runs several, a fast one may run none, and they restart with
    /// each game. Time a script by ticks to match the simulation.
    pub tick: Option<u32>,
    /// The port the bot is on (where the input manager put it).
    pub port: u8,
    /// The game's memory, to read what the bot sees.
    pub guest: &'a dyn GuestMemory,
    /// The players screen is on top and this bot's slot has no profile:
    /// pressing A signs it in (docs/game/bots.md, section 4.2).
    pub joining: bool,
}

/// Where a bot's pad comes from: one pad per frame.
pub trait PadSource: Send {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState;
}

/// A source from a closure.
impl<F: FnMut(&Frame<'_>) -> HostPadState + Send> PadSource for F {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        self(f)
    }
}

/// Frames between a joining bot's presses of A, and how long each is held.
pub const JOIN_EVERY: u64 = 60;
pub const JOIN_HOLD: u64 = 6;

/// A joining bot's pad: A for [`JOIN_HOLD`] frames in every [`JOIN_EVERY`].
pub fn join_pad(frame: u64) -> HostPadState {
    let mut pad = HostPadState::default();
    pad.buttons
        .set(HostButton::South, frame % JOIN_EVERY < JOIN_HOLD);
    pad
}

/// Timed pads: each step held for its frames, then the next; idle after
/// the last (or from the start again with `looping`).
pub struct Scripted {
    pub steps: Vec<(u64, HostPadState)>,
    pub looping: bool,
    start: Option<u64>,
}

impl Scripted {
    pub fn new(steps: Vec<(u64, HostPadState)>, looping: bool) -> Self {
        Scripted {
            steps,
            looping,
            start: None,
        }
    }
}

impl PadSource for Scripted {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let start = *self.start.get_or_insert(f.frame);
        let total: u64 = self.steps.iter().map(|s| s.0).sum();
        let mut t = f.frame - start;
        if self.looping && total > 0 {
            t %= total;
        }
        for &(n, pad) in &self.steps {
            if t < n {
                return pad;
            }
            t -= n;
        }
        HostPadState::default()
    }
}

/// A recording: one pad per frame, played once from the first frame the
/// source is asked; idle after the end.
pub struct Recorded {
    pub pads: Vec<HostPadState>,
    start: Option<u64>,
}

impl Recorded {
    pub fn new(pads: Vec<HostPadState>) -> Self {
        Recorded { pads, start: None }
    }
}

impl PadSource for Recorded {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let start = *self.start.get_or_insert(f.frame);
        let i = (f.frame - start) as usize;
        self.pads.get(i).copied().unwrap_or_default()
    }
}

/// A bot on the books.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BotHandle(pub u32);

/// What [`list`] says about one bot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotInfo {
    pub handle: BotHandle,
    pub name: String,
    /// The port asked for.
    pub wanted: u8,
    /// The port the input manager put it on, once it has.
    pub port: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Ports are 0 to 3.
    NoSuchPort(u8),
    /// Another bot asked for that port.
    PortTaken(u8),
    /// [`MAX_LOCAL`] bots already.
    Full,
    NoSuchBot(BotHandle),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoSuchPort(p) => write!(f, "port {p}: ports are 0 to 3"),
            Error::PortTaken(p) => write!(f, "port {p} already has a bot"),
            Error::Full => write!(f, "{MAX_LOCAL} local bots already"),
            Error::NoSuchBot(h) => write!(f, "no bot {}", h.0),
        }
    }
}
impl std::error::Error for Error {}

struct Bot {
    handle: BotHandle,
    name: String,
    wanted: u8,
    port: Option<u8>,
    pad: HostPadState,
    source: Box<dyn PadSource>,
}

#[derive(Default)]
struct State {
    bots: Vec<Bot>,
    next: u32,
}

/// The local bots of one machine. Cheap to clone; clones share.
#[derive(Clone, Default)]
pub struct Local(Arc<Mutex<State>>);

impl Local {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Puts a bot named `name` on `port`, driven by `source`.
    pub fn add(
        &self,
        port: u8,
        name: &str,
        source: Box<dyn PadSource>,
    ) -> Result<BotHandle, Error> {
        if port > 3 {
            return Err(Error::NoSuchPort(port));
        }
        let mut s = self.lock();
        if s.bots.iter().any(|b| b.wanted == port) {
            return Err(Error::PortTaken(port));
        }
        if s.bots.len() >= MAX_LOCAL {
            return Err(Error::Full);
        }
        let handle = BotHandle(s.next);
        s.next += 1;
        s.bots.push(Bot {
            handle,
            name: name.to_owned(),
            wanted: port,
            port: None,
            pad: HostPadState::default(),
            source,
        });
        Ok(handle)
    }

    /// Takes a bot off: its pad is unplugged at the input manager's next
    /// sync.
    pub fn remove(&self, handle: BotHandle) -> Result<(), Error> {
        let mut s = self.lock();
        let i = s
            .bots
            .iter()
            .position(|b| b.handle == handle)
            .ok_or(Error::NoSuchBot(handle))?;
        s.bots.remove(i);
        Ok(())
    }

    pub fn list(&self) -> Vec<BotInfo> {
        self.lock()
            .bots
            .iter()
            .map(|b| BotInfo {
                handle: b.handle,
                name: b.name.clone(),
                wanted: b.wanted,
                port: b.port,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().bots.is_empty()
    }

    /// Every placed bot's port, as a mask (bit `p` for port `p`).
    pub fn port_mask(&self) -> u32 {
        self.lock()
            .bots
            .iter()
            .filter_map(|b| b.port)
            .fold(0, |m, p| m | 1 << p)
    }

    /// The name of the bot on `port`.
    pub fn name_on(&self, port: u32) -> Option<String> {
        self.lock()
            .bots
            .iter()
            .find(|b| b.port.map(u32::from) == Some(port))
            .map(|b| b.name.clone())
    }

    /// One frame: asks each placed bot's source for its pad.
    /// `joining(port)` says whether that port's bot should be signing in.
    pub fn frame(&self, frame: u64, guest: &dyn GuestMemory, joining: impl Fn(u8) -> bool) {
        let tick = crate::guest::game_tick(guest);
        let mut s = self.lock();
        for b in &mut s.bots {
            let Some(port) = b.port else { continue };
            let f = Frame {
                frame,
                tick,
                port,
                guest,
                joining: joining(port),
            };
            b.pad = b.source.pad(&f);
        }
    }

    /// Bot `handle`'s current pad.
    pub fn pad(&self, handle: BotHandle) -> Option<HostPadState> {
        self.lock()
            .bots
            .iter()
            .find(|b| b.handle == handle)
            .map(|b| b.pad)
    }

    /// The input backend over these bots.
    pub fn backend(&self) -> Backend {
        Backend(self.clone())
    }
}

/// The process-wide bots the boot glue drives.
pub fn global() -> &'static Local {
    static G: OnceLock<Local> = OnceLock::new();
    G.get_or_init(Local::default)
}

/// Puts a bot on `port` of this machine (see [`Local::add`]); its name is
/// "Clanker" and the port.
pub fn add(port: u8, source: Box<dyn PadSource>) -> Result<BotHandle, Error> {
    global().add(port, &format!("Clanker {port}"), source)
}

/// Takes a bot off (see [`Local::remove`]).
pub fn remove(handle: BotHandle) -> Result<(), Error> {
    global().remove(handle)
}

/// The bots on this machine.
pub fn list() -> Vec<BotInfo> {
    global().list()
}

/// The input backend: one always-connected, synthesised pad per bot.
pub struct Backend(Local);

fn handle_of(id: HostPadId) -> Option<BotHandle> {
    id.0.checked_sub(ID_BASE)
        .and_then(|h| u32::try_from(h).ok())
        .map(BotHandle)
}

impl HostBackend for Backend {
    fn name(&self) -> &str {
        "bots"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        self.0
            .lock()
            .bots
            .iter()
            .map(|b| HostPad {
                id: HostPadId(ID_BASE + u64::from(b.handle.0)),
                name: b.name.clone(),
                state: b.pad,
                // Synthesised: no deadzone cut, no motors.
                physical: false,
                supports_rumble: false,
            })
            .collect()
    }

    fn preferred_port(&self, id: HostPadId) -> Option<usize> {
        let h = handle_of(id)?;
        self.0
            .lock()
            .bots
            .iter()
            .find(|b| b.handle == h)
            .map(|b| usize::from(b.wanted))
    }

    fn placed(&mut self, id: HostPadId, port: usize) {
        let Some(h) = handle_of(id) else { return };
        if let Some(b) = self.0.lock().bots.iter_mut().find(|b| b.handle == h) {
            b.port = u8::try_from(port).ok();
        }
    }
}

#[cfg(test)]
mod tests;
