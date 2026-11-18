//! Phantom peers (docs/game/bots.md, section 5 and stage 6): bots that are
//! remote players of a machine that does not exist, made up in memory on
//! the host.
//!
//! [`Phantoms::add`] puts a phantom on the books and returns at once. Its
//! machine and player are made later, on the game thread
//! ([`Phantoms::refresh`] finds the player once the game has made it).
//! [`Phantoms::list`] says where each one is: waiting, joined (with its
//! player's index), or failed and why.
//!
//! Each phantom has a machine id of its own ([`machine_id`]), and its
//! player is found by that id in the player records, never by the order of
//! joining.

use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::{GuestMemory, MemFault};
use engine::view::players;

use crate::guest;
use crate::local::PadSource;

/// Who a phantom is (docs/game/bots.md, section 9.1): the server's account
/// and how the lobby shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub xuid: u64,
    pub gamertag: String,
    /// Emblem foreground, background and three colours, primary and
    /// secondary armour colours, as the roster gives them.
    pub emblem: [u8; 5],
    pub colors: [u8; 2],
}

/// The most phantoms: 16 machines, one of them the host.
pub const MAX_PHANTOMS: usize = 15;

/// A phantom on the books.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhantomHandle(pub u32);

/// The first two bytes of every phantom's machine id; the other four are
/// its handle, big-endian. No real machine id starts with them: real ones
/// are the console's Ethernet address, and `C1` has the multicast bit set,
/// which no unicast adapter address carries.
pub const MACHINE_PREFIX: [u8; 2] = [0xc1, 0xa4];

/// A phantom's 6-byte machine id.
pub fn machine_id(h: PhantomHandle) -> [u8; 6] {
    let b = h.0.to_be_bytes();
    [MACHINE_PREFIX[0], MACHINE_PREFIX[1], b[0], b[1], b[2], b[3]]
}

/// The phantom a machine id belongs to, if it is one of ours.
pub fn handle_of(machine: [u8; 6]) -> Option<PhantomHandle> {
    (machine[..2] == MACHINE_PREFIX).then(|| {
        PhantomHandle(u32::from_be_bytes([
            machine[2], machine[3], machine[4], machine[5],
        ]))
    })
}

/// Where a phantom is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// On the books; the game has not made its player yet.
    Pending,
    /// The game has a player for it.
    Joined,
    /// It could not be made, and why. It stays listed until removed.
    Failed(String),
}

/// What [`Phantoms::list`] says about one phantom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhantomInfo {
    pub handle: PhantomHandle,
    pub identity: Identity,
    /// The team asked for; `None`: the host balances.
    pub team: Option<u8>,
    pub machine: [u8; 6],
    /// Its player's datum index (`player.<i>` in `sim.view`), once joined.
    pub player: Option<u32>,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Phantom machines are not made yet: the membership step of stage 6
    /// is still being traced (docs/game/bots.md, section 12).
    NotBuilt,
    /// [`MAX_PHANTOMS`] already.
    Full,
    /// A phantom with that xuid is already on the books.
    Duplicate(u64),
    /// An identity with no gamertag or a zero xuid.
    BadIdentity,
    NoSuchPhantom(PhantomHandle),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotBuilt => write!(
                f,
                "phantom peers are not built yet (docs/game/bots.md, stage 6)"
            ),
            Error::Full => write!(f, "{MAX_PHANTOMS} phantoms already"),
            Error::Duplicate(x) => write!(f, "a phantom with xuid {x:016x} already"),
            Error::BadIdentity => write!(f, "a phantom needs a gamertag and a non-zero xuid"),
            Error::NoSuchPhantom(h) => write!(f, "no phantom {}", h.0),
        }
    }
}
impl std::error::Error for Error {}

struct Phantom {
    info: PhantomInfo,
    #[allow(dead_code)] // driven once the action step lands
    source: Box<dyn PadSource>,
}

#[derive(Default)]
struct Books {
    list: Vec<Phantom>,
    next: u32,
}

/// The phantoms of one host. Cheap to clone; clones share.
#[derive(Clone, Default)]
pub struct Phantoms(Arc<Mutex<Books>>);

impl Phantoms {
    fn lock(&self) -> std::sync::MutexGuard<'_, Books> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Puts a phantom on the books, [`State::Pending`]. Never waits on the
    /// game.
    pub fn add(
        &self,
        identity: Identity,
        team: Option<u8>,
        source: Box<dyn PadSource>,
    ) -> Result<PhantomHandle, Error> {
        if identity.xuid == 0 || identity.gamertag.trim().is_empty() {
            return Err(Error::BadIdentity);
        }
        let mut b = self.lock();
        if b.list.iter().any(|p| p.info.identity.xuid == identity.xuid) {
            return Err(Error::Duplicate(identity.xuid));
        }
        if b.list.len() >= MAX_PHANTOMS {
            return Err(Error::Full);
        }
        let handle = PhantomHandle(b.next);
        b.next += 1;
        b.list.push(Phantom {
            info: PhantomInfo {
                handle,
                identity,
                team,
                machine: machine_id(handle),
                player: None,
                state: State::Pending,
            },
            source,
        });
        Ok(handle)
    }

    /// Takes a phantom off the books.
    pub fn remove(&self, handle: PhantomHandle) -> Result<(), Error> {
        let mut b = self.lock();
        let i = b
            .list
            .iter()
            .position(|p| p.info.handle == handle)
            .ok_or(Error::NoSuchPhantom(handle))?;
        b.list.remove(i);
        Ok(())
    }

    pub fn list(&self) -> Vec<PhantomInfo> {
        self.lock().list.iter().map(|p| p.info.clone()).collect()
    }

    /// Phantom `handle`'s player index, once joined.
    pub fn player(&self, handle: PhantomHandle) -> Option<u32> {
        self.lock()
            .list
            .iter()
            .find(|p| p.info.handle == handle)
            .and_then(|p| p.info.player)
    }

    /// Marks a phantom failed (the game thread could not make it).
    pub fn fail(&self, handle: PhantomHandle, why: impl Into<String>) {
        if let Some(p) = self
            .lock()
            .list
            .iter_mut()
            .find(|p| p.info.handle == handle)
        {
            p.info.state = State::Failed(why.into());
            p.info.player = None;
        }
    }

    /// Finds each phantom's player in the player records by its machine
    /// id. A phantom whose player has gone goes back to pending; a failed
    /// one stays failed.
    pub fn refresh(&self, m: &dyn GuestMemory) -> Result<(), MemFault> {
        let found = players_by_machine(m)?;
        for p in &mut self.lock().list {
            if matches!(p.info.state, State::Failed(_)) {
                continue;
            }
            let at = found
                .iter()
                .find(|(id, _)| *id == p.info.machine)
                .map(|f| f.1);
            p.info.player = at;
            p.info.state = if at.is_some() {
                State::Joined
            } else {
                State::Pending
            };
        }
        Ok(())
    }
}

/// Every listed player's machine id and datum index.
pub fn players_by_machine(m: &dyn GuestMemory) -> Result<Vec<([u8; 6], u32)>, MemFault> {
    let Some(array) = players::array(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (handle, p) in array.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        out.push((guest::player_machine(m, p.addr)?, u32::from(handle.index())));
    }
    Ok(out)
}

/// The process-wide phantoms the boot glue drives.
pub fn global() -> &'static Phantoms {
    static G: OnceLock<Phantoms> = OnceLock::new();
    G.get_or_init(Phantoms::default)
}

/// Adds a phantom peer with `identity` on `team` (`None`: the host
/// balances), driven by `source`. Refused with [`Error::NotBuilt`] until
/// the game thread makes phantom machines (stage 6's membership step).
pub fn add(
    identity: Identity,
    team: Option<u8>,
    source: Box<dyn PadSource>,
) -> Result<PhantomHandle, Error> {
    let _ = (identity, team, source);
    Err(Error::NotBuilt)
}

/// Removes a phantom peer.
pub fn remove(handle: PhantomHandle) -> Result<(), Error> {
    global().remove(handle)
}

/// The phantoms on the books.
pub fn list() -> Vec<PhantomInfo> {
    global().list()
}

/// Phantom `handle`'s player index, once joined (`sim`'s
/// `phantom_player`).
pub fn player(handle: PhantomHandle) -> Option<u32> {
    global().player(handle)
}

#[cfg(test)]
mod tests;
