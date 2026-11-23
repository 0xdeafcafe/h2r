//! One machine's end of the stream.
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use forge_doc::{Limits, OpId};

use super::{Beat, Item, MAX_AHEAD, Seat, Stamped, State, Welcome, next_in_lane};

/// A machine in a synchronous Forge session. It buffers the host's items
/// in stream order, whatever order they arrive in, and hands its game the
/// items due at each tick. The game applies each one to the world at that
/// tick: an entry through the Forge session, a carry as a move of the
/// held object.
#[derive(Clone, Debug)]
pub struct Peer {
    seat: Seat,
    limits: Limits,
    state: State,
    /// Items after the contiguous prefix.
    early: BTreeMap<u64, Stamped>,
    /// The contiguous prefix not yet due, in order, without watermarks.
    ready: VecDeque<Stamped>,
    /// The last stream item in the contiguous prefix.
    received: u64,
    /// The host's watermark: no item at or before it is still to come.
    through: u32,
    /// The last tick handed to the game.
    ran: u32,
    /// The tick of the last item taken other than a watermark.
    last: u32,
    /// The last op ID this machine allocated.
    own: u32,
}

/// Why a peer cannot go on. Every variant but [`PeerError::Wait`] ends the
/// machine's session with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerError {
    /// The host has not yet said that nothing more is due at `tick`: hold
    /// the tick.
    Wait { tick: u32, through: u32 },
    /// An item arrived for a tick this machine has already run.
    Late { n: u64, tick: u32, ran: u32 },
    /// The ticks of the stream went backwards.
    Order { n: u64 },
    /// An item too far ahead of the one missing.
    Ahead { n: u64, missing: u64 },
    /// The welcome or an item does not apply.
    Invalid { n: u64, reason: String },
    /// A tick earlier than one already run.
    Rewind { tick: u32, ran: u32 },
}

impl fmt::Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wait { tick, through } => {
                write!(
                    f,
                    "Forge waits at tick {tick}: the host has sent up to {through}."
                )
            }
            Self::Late { n, tick, ran } => write!(
                f,
                "Forge item {n} is for tick {tick}, but this machine has run to {ran}."
            ),
            Self::Order { n } => write!(f, "Forge item {n} goes back in time."),
            Self::Ahead { n, missing } => {
                write!(f, "Forge item {n} is too far ahead of item {missing}.")
            }
            Self::Invalid { n, reason } => write!(f, "Forge item {n}: {reason}"),
            Self::Rewind { tick, ran } => {
                write!(f, "Forge tick {tick} is before tick {ran}, already run.")
            }
        }
    }
}

impl std::error::Error for PeerError {}

impl Peer {
    /// Start from the host's welcome: fold the checkpoint with its entries,
    /// check the head, and take the items still to apply.
    pub fn join(welcome: Welcome, limits: Limits) -> Result<Self, PeerError> {
        let state =
            State::from_welcome(&welcome, &limits).map_err(|reason| PeerError::Invalid {
                n: welcome.n,
                reason,
            })?;
        let mut peer = Self {
            seat: welcome.seat,
            limits,
            received: state.n(),
            state,
            early: BTreeMap::new(),
            ready: VecDeque::new(),
            through: welcome.through,
            ran: welcome.tick,
            last: welcome.tick,
            own: 0,
        };
        for item in welcome.pending {
            peer.receive(item)?;
        }
        Ok(peer)
    }

    pub fn seat(&self) -> Seat {
        self.seat
    }

    /// The session as of the last item handed to the game.
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn through(&self) -> u32 {
        self.through
    }

    /// The number of the next stream item this peer is missing.
    pub fn next_n(&self) -> u64 {
        self.received + 1
    }

    /// Say that nothing at or before `tick` is still to come, as the host's
    /// watermark does. A film's replay knows it: the film holds every item
    /// of the tick it has reached.
    pub fn release_through(&mut self, tick: u32) {
        self.through = self.through.max(tick);
    }

    /// The keep-alive's position.
    pub fn beat(&self) -> Beat {
        Beat {
            head: self.state.head(),
            n: self.state.n(),
        }
    }

    /// Take one item of the host's stream. A copy of one already taken is
    /// ignored.
    pub fn receive(&mut self, item: Stamped) -> Result<(), PeerError> {
        if item.n <= self.received || self.early.contains_key(&item.n) {
            return Ok(());
        }
        let missing = self.received + 1;
        if item.n - missing >= MAX_AHEAD {
            return Err(PeerError::Ahead { n: item.n, missing });
        }
        self.early.insert(item.n, item);
        while let Some(item) = self.early.remove(&(self.received + 1)) {
            self.received = item.n;
            if item.item == Item::Through {
                self.through = self.through.max(item.tick);
                // Keep its place in the stream: the state counts it.
                self.ready.push_back(item);
                continue;
            }
            if item.tick <= self.ran {
                return Err(PeerError::Late {
                    n: item.n,
                    tick: item.tick,
                    ran: self.ran,
                });
            }
            if item.tick < self.last {
                return Err(PeerError::Order { n: item.n });
            }
            self.last = item.tick;
            self.ready.push_back(item);
        }
        Ok(())
    }

    /// The items due at `tick`, applied to the state in stream order, for
    /// the game to apply to the world before the tick runs. `Wait` until
    /// the host's watermark reaches `tick`.
    pub fn due(&mut self, tick: u32) -> Result<Vec<Stamped>, PeerError> {
        if tick < self.ran {
            return Err(PeerError::Rewind {
                tick,
                ran: self.ran,
            });
        }
        if tick > self.through {
            return Err(PeerError::Wait {
                tick,
                through: self.through,
            });
        }
        let mut due = vec![];
        while self
            .ready
            .front()
            .is_some_and(|item| item.tick <= tick || item.item == Item::Through)
        {
            let item = self.ready.pop_front().expect("front exists");
            self.state
                .apply(&item, &self.limits)
                .map_err(|reason| PeerError::Invalid { n: item.n, reason })?;
            if item.item != Item::Through {
                due.push(item);
            }
        }
        self.ran = tick;
        Ok(due)
    }

    /// New op IDs for this seat's next edit, in its lane above every ID
    /// the session has named and every one this machine gave out.
    pub fn op_ids(&mut self, count: usize) -> Option<Vec<OpId>> {
        let mut above = self.state.max_id().max(self.own);
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            let id = next_in_lane(self.seat, above)?;
            above = id.0;
            ids.push(id);
        }
        self.own = above;
        Some(ids)
    }
}
