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
//! `XInputGetState` like any other.
//!
//! **Leaving.** [`Local::remove`] does not unplug the pad: a pad pulled
//! during a game makes the game pause on "controller disconnected" for a
//! port no human holds (the user's run, docs/game/bots.md §4.2). The bot
//! leaves the books at once ([`list`] no longer has it) but stays plugged
//! with an idle pad. The boot glue takes it the rest of the way with
//! [`leave_step`]: once no game runs it signs the bot's slot out altogether
//! (`SLOT_SIGN_OUT`: off Xbox Live, its local user freed, so the lobby's
//! session drops its player), and once the slot is empty it unplugs the
//! pad ([`Local::unplug`]). A bot added again on that port before then takes
//! the leaving bot's pad back, so the game never sees it go.
//!
//! The free functions work on one process-wide [`Local`] ([`global`]),
//! which the boot glue drives. Tests make their own.

use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use input::HostBackend;
use input::host::{HostPad, HostPadId, HostPadState};

/// The most local bots: four ports, one human.
pub const MAX_LOCAL: usize = 3;

/// The first [`HostPadId`] of a bot pad; the bot with handle `h` is
/// `ID_BASE + h`.
pub const ID_BASE: u64 = 0xc1a4_0000;

/// What a source sees each frame.
pub struct Frame<'a> {
    /// Input sample number. Local pads use rendered frames, including menus.
    /// A phantom advances once per simulation sample, including catch-up ticks.
    pub frame: u64,
    /// The game's tick (30 a second) while a game is loaded, `None` in the
    /// menus ([`crate::guest::game_tick`]). Ticks are not frames: a slow
    /// frame runs several, a fast one may run none, and they restart with
    /// each game. Time a script by ticks to match the simulation.
    pub tick: Option<u32>,
    /// The port the bot is on (where the input manager put it). For a
    /// phantom, which has no port, 4.
    pub port: u8,
    /// A phantom's machine id, by which its player is found; `None` for a
    /// local bot, whose player is the one on `port`.
    pub machine: Option<[u8; 6]>,
    /// The game's memory, to read what the bot sees.
    pub guest: &'a dyn GuestMemory,
    /// The players screen is on top and this bot's slot has no profile:
    /// the glue is signing it in itself (docs/game/bots.md, section 4.2), so
    /// a source should press nothing. A bot never presses a button to join:
    /// any press from a controller with no profile opens the game's profile
    /// chooser, which picks the human's profile.
    pub joining: bool,
}

/// Where a bot's pad comes from: one pad per frame.
pub trait PadSource: Send {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState;

    fn brain_view(&self) -> Option<crate::BrainView> {
        None
    }

    /// Roster personality (seed, taunt). Scripted pads have no brain to update.
    fn set_roster_personality(&mut self, _personality: Option<(u64, f32)>) {}
}

/// A source from a closure.
impl<F: FnMut(&Frame<'_>) -> HostPadState + Send> PadSource for F {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        self(f)
    }
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
    /// [`MAX_LOCAL`] bots already (or the capacity
    /// [`Local::set_capacity`] gave).
    Full,
    NoSuchBot(BotHandle),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoSuchPort(p) => write!(f, "port {p}: ports are 0 to 3"),
            Error::PortTaken(p) => write!(f, "port {p} already has a bot"),
            Error::Full => f.write_str("every local bot place is taken"),
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
    /// Taken off the books; plugged with an idle pad until the glue has
    /// signed it out ([`leave_step`]).
    leaving: bool,
    /// The bot account it is on Xbox Live as, once the server leased one
    /// (docs/game/bots.md §9.5); `None`: the human's guest, or offline.
    account: Option<crate::guest::BotAccount>,
}

#[derive(Default)]
struct State {
    bots: Vec<Bot>,
    next: u32,
    /// [`Local::set_capacity`]; `None`: [`MAX_LOCAL`].
    capacity: Option<usize>,
}

/// The local bots of one machine. Cheap to clone; clones share.
#[derive(Clone, Default)]
pub struct Local(Arc<Mutex<State>>);

impl Local {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How many bots it takes: [`MAX_LOCAL`], leaving port 0 to a human,
    /// unless set otherwise.
    pub fn capacity(&self) -> usize {
        self.lock().capacity.unwrap_or(MAX_LOCAL)
    }

    /// Takes up to `n` bots (1 to 4). Sim mode has no human and no
    /// keyboard pad, so all four ports can be bots
    /// (docs/runtime/sim.md §4).
    pub fn set_capacity(&self, n: usize) {
        self.lock().capacity = Some(n.clamp(1, 4));
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
        if s.bots.iter().any(|b| b.wanted == port && !b.leaving) {
            return Err(Error::PortTaken(port));
        }
        // A bot still leaving that port: the new one takes its pad, which
        // stays plugged (and signed in, if it was).
        if let Some(b) = s.bots.iter_mut().find(|b| b.wanted == port && b.leaving) {
            b.name = name.to_owned();
            b.source = source;
            b.leaving = false;
            b.pad = HostPadState::default();
            b.account = None;
            return Ok(b.handle);
        }
        if s.bots.len() >= s.capacity.unwrap_or(MAX_LOCAL) {
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
            leaving: false,
            account: None,
        });
        Ok(handle)
    }

    /// Gives the bot `handle` the bot account it goes on Xbox Live as
    /// (`None`: none, so it is the human's guest there).
    pub fn set_account(
        &self,
        handle: BotHandle,
        account: Option<crate::guest::BotAccount>,
    ) -> Result<(), Error> {
        self.set_roster_account(handle, account, None)
    }

    pub fn set_roster_account(
        &self,
        handle: BotHandle,
        account: Option<crate::guest::BotAccount>,
        personality: Option<(u64, f32)>,
    ) -> Result<(), Error> {
        let mut s = self.lock();
        let b = s
            .bots
            .iter_mut()
            .find(|b| b.handle == handle && !b.leaving)
            .ok_or(Error::NoSuchBot(handle))?;
        b.source.set_roster_personality(personality);
        b.account = account;
        Ok(())
    }

    /// The bot account of the bot on `port` (on the books), if it has one.
    pub fn account_on(&self, port: u32) -> Option<crate::guest::BotAccount> {
        self.lock()
            .bots
            .iter()
            .filter(|b| !b.leaving)
            .find(|b| b.port.map(u32::from) == Some(port))
            .and_then(|b| b.account.clone())
    }

    /// The bot account of the bot `handle`, if it has one.
    pub fn account_of(&self, handle: BotHandle) -> Option<crate::guest::BotAccount> {
        self.lock()
            .bots
            .iter()
            .find(|b| b.handle == handle && !b.leaving)
            .and_then(|b| b.account.clone())
    }

    pub fn brain_view(&self, handle: BotHandle) -> Option<crate::BrainView> {
        self.lock()
            .bots
            .iter()
            .find(|b| b.handle == handle && !b.leaving)?
            .source
            .brain_view()
    }

    /// Takes a bot off the books. Its pad stays plugged, idle, until the
    /// glue signs it out and unplugs it ([`leave_step`], [`Local::unplug`]);
    /// pulling it at once would show "controller disconnected" in a game.
    pub fn remove(&self, handle: BotHandle) -> Result<(), Error> {
        let mut s = self.lock();
        let b = s
            .bots
            .iter_mut()
            .find(|b| b.handle == handle && !b.leaving)
            .ok_or(Error::NoSuchBot(handle))?;
        b.leaving = true;
        b.pad = HostPadState::default();
        Ok(())
    }

    /// The bots leaving ([`Local::remove`]) and the port each is on.
    pub fn leaving(&self) -> Vec<(BotHandle, Option<u8>)> {
        self.lock()
            .bots
            .iter()
            .filter(|b| b.leaving)
            .map(|b| (b.handle, b.port))
            .collect()
    }

    /// Unplugs a leaving bot's pad (at the input manager's next sync). A
    /// bot on the books stays: [`Local::remove`] it first.
    pub fn unplug(&self, handle: BotHandle) -> bool {
        let mut s = self.lock();
        let before = s.bots.len();
        s.bots.retain(|b| !(b.handle == handle && b.leaving));
        s.bots.len() != before
    }

    /// The bots on the books (not those leaving).
    pub fn list(&self) -> Vec<BotInfo> {
        self.lock()
            .bots
            .iter()
            .filter(|b| !b.leaving)
            .map(|b| BotInfo {
                handle: b.handle,
                name: b.name.clone(),
                wanted: b.wanted,
                port: b.port,
            })
            .collect()
    }

    /// No bot on the books (leaving ones may still be plugged).
    pub fn is_empty(&self) -> bool {
        self.lock().bots.iter().all(|b| b.leaving)
    }

    /// Every placed bot's port, as a mask (bit `p` for port `p`), leaving
    /// ones included: their players are still in the game until they are
    /// signed out, so their views stay hidden.
    pub fn port_mask(&self) -> u32 {
        self.lock()
            .bots
            .iter()
            .filter_map(|b| b.port)
            .fold(0, |m, p| m | 1 << p)
    }

    /// The placed bots on the books, as a port mask: the ones that may
    /// sign in.
    pub fn joining_mask(&self) -> u32 {
        self.lock()
            .bots
            .iter()
            .filter(|b| !b.leaving)
            .filter_map(|b| b.port)
            .fold(0, |m, p| m | 1 << p)
    }

    /// The name of the bot on `port` (on the books).
    pub fn name_on(&self, port: u32) -> Option<String> {
        self.lock()
            .bots
            .iter()
            .filter(|b| !b.leaving)
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
            if b.leaving {
                b.pad = HostPadState::default();
                continue;
            }
            let f = Frame {
                frame,
                tick,
                port,
                machine: None,
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

/// What the glue does for the leaving bots this frame ([`leave_step`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaveStep {
    /// Ports whose slot to sign out (`SLOT_SIGN_OUT`), first.
    pub sign_out: Vec<u8>,
    /// Bots whose pad to unplug: their slot is empty already.
    pub unplug: Vec<BotHandle>,
}

/// The next step for the `leaving` bots ([`Local::leaving`]): nothing while
/// a game runs (`in_game`: the bot's player is in it, and an unplugged pad
/// pauses the game); else sign out each bot whose port's slot still holds
/// a profile or Xbox Live (`held`, [`crate::guest::slot_held`]), and
/// unplug each whose slot holds neither (or that was never placed). A bot
/// is signed out one frame and unplugged a later one, so the game never
/// sees a signed-in slot without its pad.
pub fn leave_step(
    in_game: bool,
    leaving: &[(BotHandle, Option<u8>)],
    held: impl Fn(u8) -> bool,
) -> LeaveStep {
    let mut step = LeaveStep::default();
    if in_game {
        return step;
    }
    for &(h, port) in leaving {
        match port {
            Some(p) if held(p) => step.sign_out.push(p),
            _ => step.unplug.push(h),
        }
    }
    step
}

/// The bots' ports (`joining`, a mask: [`Local::joining_mask`]) whose slot
/// holds a saved profile file (`saved`, [`crate::guest::saved_profile`]),
/// to sign out (`SLOT_SIGN_OUT`) while no game runs (`in_game`). A bot is
/// only ever signed in with no file, so a file there is a real profile a
/// chooser gave the bot's controller (a stray A): the slot keeps it in
/// use, so every chooser hides it, and the save pass would write the bot's
/// block over it. Signed out, the profile is offered again and the join
/// gate signs the bot in with its own block. Out of a game only: the
/// bot's player is in it.
pub fn release_step(in_game: bool, joining: u32, saved: impl Fn(u8) -> bool) -> Vec<u8> {
    if in_game {
        return Vec::new();
    }
    (0..4u8)
        .filter(|&p| joining & 1 << p != 0 && saved(p))
        .collect()
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
