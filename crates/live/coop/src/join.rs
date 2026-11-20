//! Join in progress (docs/live/live-coop.md section 14): a friend joins a
//! running co-op campaign without anyone reverting.
//!
//! The newcomer has already loaded the host's level. It asks; the host
//! holds its tick at the next boundary, snapshots the game state, and sends
//! it in chunks; the newcomer restores it and says so; the host lets the
//! newcomer's player into the next tick's update and resumes. If anything
//! fails or stalls the host resumes without the player and the newcomer
//! goes back to the lobby, so a failed join changes nothing for the others.
//!
//! Like the rest of the crate this is pure: the caller says what time it
//! is, hands in messages and the snapshot, and does what the [`HostOut`] and
//! [`JoinOut`] lists say. Nothing here knows what the state bytes hold
//! (the caller compresses them).
//!
//! Layout, little endian: magic `h2cj`, version (u8), session (u32), kind
//! (u8), nonce (u32), then per kind:
//! - request: nothing;
//! - accept: tick (u32), slot (u8), bsp (u16), length (u32), chunk count
//!   (u32), checksum (u64), level name length (u8) and that many bytes;
//! - refuse, abort: reason (u8);
//! - chunk: index (u32), length (u16), that many bytes;
//! - ack: next chunk wanted (u32);
//! - ready: tick restored (u32).

pub const MAGIC: [u8; 4] = *b"h2cj";
pub const VERSION: u8 = 1;
/// State bytes per chunk. A chunk datagram stays under the relay's 1200
/// byte cap and the usual MTU.
pub const CHUNK: usize = 1024;
/// The largest chunk datagram.
pub const MAX_CHUNK_DATAGRAM: usize = HEADER + 4 + 2 + CHUNK;
/// Chunks sent past the newcomer's acknowledgement. Under the relay's queue
/// of 256 messages per member.
pub const WINDOW: u32 = 64;
/// The most (compressed) state a newcomer takes: twice the game state
/// block (0x3fe000 bytes) in case compression gains nothing.
pub const MAX_STATE: u32 = 2 * 0x3f_e000;
/// Level names longer than this are refused (the game's own are short).
pub const MAX_LEVEL: usize = 63;
/// How often an unanswered message is sent again.
pub const RESEND_MS: u64 = 250;
/// No progress for this long ends the join.
pub const STALL_MS: u64 = 3_000;
/// The longest the host holds its tick for one join.
pub const HOLD_MS: u64 = 15_000;
/// How long the newcomer asks before it gives up.
pub const ASK_MS: u64 = 5_000;

const HEADER: usize = 4 + 1 + 4 + 1 + 4;

/// Why the host said no.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `coop.join_in_progress` is off on the host.
    Off = 1,
    /// Every player slot is taken.
    Full = 2,
    /// Another join is under way.
    Busy = 3,
    /// The host is not in a level.
    NotInGame = 4,
}

/// Why a join ended part way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abort {
    /// No progress for [`STALL_MS`], or the hold passed [`HOLD_MS`].
    Timeout = 1,
    /// The state did not match its checksum.
    Checksum = 2,
    /// The newcomer could not restore it.
    Restore = 3,
    /// Either side gave up (left the session, quit).
    Cancelled = 4,
}

impl Refusal {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Off,
            2 => Self::Full,
            3 => Self::Busy,
            4 => Self::NotInGame,
            _ => return None,
        })
    }
    pub fn says(self) -> &'static str {
        match self {
            Self::Off => "the host has join in progress off",
            Self::Full => "the game is full",
            Self::Busy => "someone else is joining",
            Self::NotInGame => "the host is not in a level",
        }
    }
}

impl Abort {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Timeout,
            2 => Self::Checksum,
            3 => Self::Restore,
            4 => Self::Cancelled,
            _ => return None,
        })
    }
    pub fn says(self) -> &'static str {
        match self {
            Self::Timeout => "timed out",
            Self::Checksum => "the state arrived damaged",
            Self::Restore => "the state did not restore",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What the host's accept promises: the state of the end of `tick`, for
/// player `slot`, on `level` and its `bsp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub tick: u32,
    pub slot: u8,
    pub level: String,
    pub bsp: u16,
    pub len: u32,
    pub chunks: u32,
    pub sum: u64,
}

impl Plan {
    fn new(tick: u32, slot: u8, level: &str, bsp: u16, state: &[u8]) -> Plan {
        Plan {
            tick,
            slot,
            level: level.into(),
            bsp,
            len: state.len() as u32,
            chunks: chunks_for(state.len()),
            sum: checksum(state),
        }
    }

    /// Whether the plan is one a newcomer can take.
    fn sane(&self) -> bool {
        self.len <= MAX_STATE
            && self.chunks == chunks_for(self.len as usize)
            && self.level.len() <= MAX_LEVEL
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Request,
    Accept(Plan),
    Refuse(Refusal),
    Chunk { index: u32, data: Vec<u8> },
    Ack { next: u32 },
    Ready { tick: u32 },
    Abort(Abort),
}

/// One join datagram: the session it belongs to, the join it is about and
/// the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub session: u32,
    pub nonce: u32,
    pub msg: Msg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    Malformed,
    Version(u8),
}

/// The chunk count for `len` state bytes (at least one, so an accept is
/// always followed by something to acknowledge).
pub fn chunks_for(len: usize) -> u32 {
    len.div_ceil(CHUNK).max(1) as u32
}

/// FNV-1a, 64 bit: catches a damaged or misordered transfer.
pub fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Datagram {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + 32);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.session.to_le_bytes());
        let kind = match &self.msg {
            Msg::Request => 1,
            Msg::Accept(_) => 2,
            Msg::Refuse(_) => 3,
            Msg::Chunk { .. } => 4,
            Msg::Ack { .. } => 5,
            Msg::Ready { .. } => 6,
            Msg::Abort(_) => 7,
        };
        out.push(kind);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        match &self.msg {
            Msg::Request => {}
            Msg::Accept(p) => {
                out.extend_from_slice(&p.tick.to_le_bytes());
                out.push(p.slot);
                out.extend_from_slice(&p.bsp.to_le_bytes());
                out.extend_from_slice(&p.len.to_le_bytes());
                out.extend_from_slice(&p.chunks.to_le_bytes());
                out.extend_from_slice(&p.sum.to_le_bytes());
                let name = &p.level.as_bytes()[..p.level.len().min(MAX_LEVEL)];
                out.push(name.len() as u8);
                out.extend_from_slice(name);
            }
            Msg::Refuse(r) => out.push(*r as u8),
            Msg::Abort(a) => out.push(*a as u8),
            Msg::Chunk { index, data } => {
                let data = &data[..data.len().min(CHUNK)];
                out.extend_from_slice(&index.to_le_bytes());
                out.extend_from_slice(&(data.len() as u16).to_le_bytes());
                out.extend_from_slice(data);
            }
            Msg::Ack { next } => out.extend_from_slice(&next.to_le_bytes()),
            Msg::Ready { tick } => out.extend_from_slice(&tick.to_le_bytes()),
        }
        out
    }

    /// Whether `bytes` is a join datagram at all (the co-op link carries
    /// the pad datagrams too).
    pub fn is_join(bytes: &[u8]) -> bool {
        bytes.starts_with(&MAGIC)
    }

    pub fn decode(bytes: &[u8]) -> Result<Datagram, WireError> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(WireError::Malformed);
        }
        let version = r.u8()?;
        if version != VERSION {
            return Err(WireError::Version(version));
        }
        let session = r.u32()?;
        let kind = r.u8()?;
        let nonce = r.u32()?;
        let msg = match kind {
            1 => Msg::Request,
            2 => {
                let tick = r.u32()?;
                let slot = r.u8()?;
                let bsp = r.u16()?;
                let (len, chunks, sum) = (r.u32()?, r.u32()?, r.u64()?);
                let n = usize::from(r.u8()?);
                if n > MAX_LEVEL {
                    return Err(WireError::Malformed);
                }
                let level = std::str::from_utf8(r.take(n)?)
                    .map_err(|_| WireError::Malformed)?
                    .to_string();
                Msg::Accept(Plan {
                    tick,
                    slot,
                    level,
                    bsp,
                    len,
                    chunks,
                    sum,
                })
            }
            3 => Msg::Refuse(Refusal::from_u8(r.u8()?).ok_or(WireError::Malformed)?),
            4 => {
                let index = r.u32()?;
                let n = usize::from(r.u16()?);
                if n > CHUNK {
                    return Err(WireError::Malformed);
                }
                Msg::Chunk {
                    index,
                    data: r.take(n)?.to_vec(),
                }
            }
            5 => Msg::Ack { next: r.u32()? },
            6 => Msg::Ready { tick: r.u32()? },
            7 => Msg::Abort(Abort::from_u8(r.u8()?).ok_or(WireError::Malformed)?),
            _ => return Err(WireError::Malformed),
        };
        if r.at != bytes.len() {
            return Err(WireError::Malformed);
        }
        Ok(Datagram {
            session,
            nonce,
            msg,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let s = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or(WireError::Malformed)?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// What the host's game side must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostOut {
    /// Send this to the newcomer.
    Send(Msg),
    /// Stop at the next tick boundary and stay there: no update goes out
    /// until [`HostOut::Resume`].
    Hold,
    /// At the held boundary, snapshot the game state (compressed) and hand
    /// it to [`Host::snapshot`]. The snapshot is taken before the player is
    /// added, so it is the same state every console already has.
    TakeSnapshot { slot: u8 },
    /// Put the newcomer's player in slot `slot` into the update of tick
    /// `tick` (the one after the snapshot), so every console adds it on the
    /// same tick.
    Admit { slot: u8, tick: u32 },
    /// The join ended without a player: nothing to undo.
    Failed { why: Abort },
    /// Go on ticking.
    Resume,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostState {
    Idle,
    Snapshot {
        nonce: u32,
        slot: u8,
        since: u64,
    },
    Sending {
        nonce: u32,
        plan: Plan,
        state: Vec<u8>,
        acked: u32,
        sent_to: u32,
        since: u64,
        progress: u64,
        sent_at: u64,
    },
    Ready {
        nonce: u32,
        plan: Plan,
        since: u64,
        progress: u64,
        sent_at: u64,
    },
}

/// What the host knows of its own game when a request comes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostGame {
    pub in_level: bool,
    /// The lowest player slot nobody holds, if any.
    pub free_slot: Option<u8>,
}

/// The host's end of one join at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    enabled: bool,
    state: HostState,
    /// The last join admitted, so a repeated ready is not a new one.
    admitted: Option<u32>,
}

impl Host {
    pub fn new(enabled: bool) -> Host {
        Host {
            enabled,
            state: HostState::Idle,
            admitted: None,
        }
    }

    /// Whether the host's tick is held for a join.
    pub fn holding(&self) -> bool {
        self.state != HostState::Idle
    }

    pub fn on_msg(&mut self, now: u64, nonce: u32, msg: Msg, game: HostGame) -> Vec<HostOut> {
        let current = match &self.state {
            HostState::Idle => None,
            HostState::Snapshot { nonce, .. }
            | HostState::Sending { nonce, .. }
            | HostState::Ready { nonce, .. } => Some(*nonce),
        };
        match msg {
            Msg::Request if current == Some(nonce) || self.admitted == Some(nonce) => vec![],
            Msg::Request => {
                let refuse = |r| vec![HostOut::Send(Msg::Refuse(r))];
                if !self.enabled {
                    return refuse(Refusal::Off);
                }
                if current.is_some() {
                    return refuse(Refusal::Busy);
                }
                if !game.in_level {
                    return refuse(Refusal::NotInGame);
                }
                let Some(slot) = game.free_slot else {
                    return refuse(Refusal::Full);
                };
                self.state = HostState::Snapshot {
                    nonce,
                    slot,
                    since: now,
                };
                vec![HostOut::Hold, HostOut::TakeSnapshot { slot }]
            }
            _ if current != Some(nonce) => vec![],
            Msg::Ack { next } => self.acked(now, next),
            Msg::Ready { tick } => match &self.state {
                HostState::Ready { plan, .. } if plan.tick == tick => {
                    let out = vec![
                        HostOut::Admit {
                            slot: plan.slot,
                            tick: tick.wrapping_add(1),
                        },
                        HostOut::Resume,
                    ];
                    self.admitted = Some(nonce);
                    self.state = HostState::Idle;
                    out
                }
                _ => vec![],
            },
            Msg::Abort(why) => self.fail(why, false),
            _ => vec![],
        }
    }

    /// The snapshot [`HostOut::TakeSnapshot`] asked for, of the end of
    /// `tick` on `level`/`bsp`. Sends the accept and the first window.
    pub fn snapshot(
        &mut self,
        now: u64,
        tick: u32,
        level: &str,
        bsp: u16,
        state: Vec<u8>,
    ) -> Vec<HostOut> {
        let HostState::Snapshot { nonce, slot, since } = self.state else {
            return vec![];
        };
        let plan = Plan::new(tick, slot, level, bsp, &state);
        if !plan.sane() {
            return self.fail(Abort::Cancelled, true);
        }
        let mut out = vec![HostOut::Send(Msg::Accept(plan.clone()))];
        self.state = HostState::Sending {
            nonce,
            plan,
            state,
            acked: 0,
            sent_to: 0,
            since,
            progress: now,
            sent_at: now,
        };
        self.send_window(&mut out);
        out
    }

    /// The host could not snapshot (or its game left the level).
    pub fn cancel(&mut self) -> Vec<HostOut> {
        if self.holding() {
            self.fail(Abort::Cancelled, true)
        } else {
            vec![]
        }
    }

    /// Resends and timeouts.
    pub fn poll(&mut self, now: u64) -> Vec<HostOut> {
        let (since, progress, sent_at) = match &self.state {
            HostState::Idle => return vec![],
            HostState::Snapshot { since, .. } => (*since, *since, now),
            HostState::Sending {
                since,
                progress,
                sent_at,
                ..
            }
            | HostState::Ready {
                since,
                progress,
                sent_at,
                ..
            } => (*since, *progress, *sent_at),
        };
        if now.saturating_sub(since) > HOLD_MS || now.saturating_sub(progress) > STALL_MS {
            return self.fail(Abort::Timeout, true);
        }
        if now.saturating_sub(sent_at) < RESEND_MS {
            return vec![];
        }
        let mut out = Vec::new();
        match &mut self.state {
            HostState::Sending {
                plan,
                acked,
                sent_to,
                sent_at,
                ..
            } => {
                if *acked == 0 {
                    out.push(HostOut::Send(Msg::Accept(plan.clone())));
                }
                *sent_to = *acked;
                *sent_at = now;
                self.send_window(&mut out);
            }
            HostState::Ready { plan, sent_at, .. } => {
                // The last ack may be lost: the final chunk again asks the
                // newcomer to say where it is.
                *sent_at = now;
                let last = plan.chunks - 1;
                if let Some(c) = self.chunk(last) {
                    out.push(HostOut::Send(c));
                }
            }
            _ => {}
        }
        out
    }

    fn acked(&mut self, now: u64, next: u32) -> Vec<HostOut> {
        // While the newcomer restores, its answers to the repeated last
        // chunk show it is still there.
        if let HostState::Ready { plan, progress, .. } = &mut self.state {
            if next == plan.chunks {
                *progress = now;
            }
            return vec![];
        }
        let HostState::Sending {
            nonce,
            plan,
            acked,
            sent_to,
            since,
            progress,
            ..
        } = &mut self.state
        else {
            return vec![];
        };
        if next <= *acked || next > plan.chunks {
            return vec![];
        }
        *acked = next;
        *progress = now;
        *sent_to = (*sent_to).max(next);
        if next == plan.chunks {
            self.state = HostState::Ready {
                nonce: *nonce,
                plan: plan.clone(),
                since: *since,
                progress: now,
                sent_at: now,
            };
            return vec![];
        }
        let mut out = Vec::new();
        self.send_window(&mut out);
        out
    }

    fn send_window(&mut self, out: &mut Vec<HostOut>) {
        let HostState::Sending {
            plan,
            acked,
            sent_to,
            ..
        } = &self.state
        else {
            return;
        };
        let end = acked.saturating_add(WINDOW).min(plan.chunks);
        let from = *sent_to;
        for i in from..end {
            if let Some(c) = self.chunk(i) {
                out.push(HostOut::Send(c));
            }
        }
        if let HostState::Sending { sent_to, .. } = &mut self.state {
            *sent_to = end.max(*sent_to);
        }
    }

    fn chunk(&self, i: u32) -> Option<Msg> {
        let state = match &self.state {
            HostState::Sending { state, .. } => state,
            HostState::Ready { .. } => return self.ready_chunk(i),
            _ => return None,
        };
        let at = i as usize * CHUNK;
        let data = state.get(at..(at + CHUNK).min(state.len()))?.to_vec();
        Some(Msg::Chunk { index: i, data })
    }

    /// In the ready state the bytes are gone; the repeat of the last chunk
    /// carries none and only asks for an ack.
    fn ready_chunk(&self, i: u32) -> Option<Msg> {
        Some(Msg::Chunk {
            index: i,
            data: Vec::new(),
        })
    }

    fn fail(&mut self, why: Abort, tell: bool) -> Vec<HostOut> {
        let was = std::mem::replace(&mut self.state, HostState::Idle);
        if was == HostState::Idle {
            return vec![];
        }
        let mut out = Vec::new();
        if tell {
            out.push(HostOut::Send(Msg::Abort(why)));
        }
        out.push(HostOut::Failed { why });
        out.push(HostOut::Resume);
        out
    }
}

/// Why the newcomer went back to the lobby.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinFail {
    Refused(Refusal),
    Aborted(Abort),
    /// The host never answered the request.
    NoAnswer,
}

impl JoinFail {
    pub fn says(self) -> String {
        match self {
            Self::Refused(r) => format!("refused: {}", r.says()),
            Self::Aborted(a) => format!("join ended: {}", a.says()),
            Self::NoAnswer => "the host did not answer".into(),
        }
    }
}

/// What the newcomer's game side must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinOut {
    Send(Msg),
    /// Restore `state` (of the end of `plan.tick`) over the level already
    /// loaded, then call [`Joiner::restored`].
    Restore {
        plan: Plan,
        state: Vec<u8>,
    },
    /// Restored and the host told: run as a sync-client from `tick` with
    /// the local player in slot `slot`.
    Joined {
        slot: u8,
        tick: u32,
    },
    BackToLobby(JoinFail),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum JoinState {
    Asking {
        since: u64,
        sent_at: u64,
    },
    Receiving {
        plan: Plan,
        got: Vec<Option<Vec<u8>>>,
        next: u32,
        progress: u64,
    },
    Restoring {
        plan: Plan,
        since: u64,
    },
    /// Told the host; repeats the ready until the first update arrives.
    Joined {
        plan: Plan,
        sent_at: u64,
    },
    Done,
}

/// The newcomer's end of a join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joiner {
    pub nonce: u32,
    state: JoinState,
}

impl Joiner {
    /// Asks to join (the level already loaded).
    pub fn start(now: u64, nonce: u32) -> (Joiner, Vec<JoinOut>) {
        let j = Joiner {
            nonce,
            state: JoinState::Asking {
                since: now,
                sent_at: now,
            },
        };
        (j, vec![JoinOut::Send(Msg::Request)])
    }

    pub fn done(&self) -> bool {
        self.state == JoinState::Done
    }

    pub fn on_msg(&mut self, now: u64, nonce: u32, msg: Msg) -> Vec<JoinOut> {
        if nonce != self.nonce || self.done() {
            return vec![];
        }
        match msg {
            Msg::Accept(plan) => {
                if !matches!(self.state, JoinState::Asking { .. }) {
                    return vec![];
                }
                if !plan.sane() {
                    return self.fail(JoinFail::Aborted(Abort::Checksum), true);
                }
                self.state = JoinState::Receiving {
                    got: vec![None; plan.chunks as usize],
                    plan,
                    next: 0,
                    progress: now,
                };
                vec![JoinOut::Send(Msg::Ack { next: 0 })]
            }
            Msg::Refuse(r) => self.fail(JoinFail::Refused(r), false),
            Msg::Abort(a) => self.fail(JoinFail::Aborted(a), false),
            Msg::Chunk { index, data } => self.chunk(now, index, data),
            _ => vec![],
        }
    }

    fn chunk(&mut self, now: u64, index: u32, data: Vec<u8>) -> Vec<JoinOut> {
        let JoinState::Receiving {
            plan,
            got,
            next,
            progress,
        } = &mut self.state
        else {
            return match &self.state {
                // The host missed the last ack.
                JoinState::Restoring { plan, .. } | JoinState::Joined { plan, .. } => {
                    vec![JoinOut::Send(Msg::Ack { next: plan.chunks })]
                }
                _ => vec![],
            };
        };
        let want = if index + 1 == plan.chunks {
            plan.len as usize - index as usize * CHUNK
        } else {
            CHUNK
        };
        if index < plan.chunks && data.len() == want && got[index as usize].is_none() {
            got[index as usize] = Some(data);
            let before = *next;
            while (*next as usize) < got.len() && got[*next as usize].is_some() {
                *next += 1;
            }
            if *next > before {
                *progress = now;
            }
        }
        let next = *next;
        if next < plan.chunks {
            return vec![JoinOut::Send(Msg::Ack { next })];
        }
        let state: Vec<u8> = got.iter_mut().flat_map(|c| c.take().unwrap()).collect();
        let plan = plan.clone();
        if state.len() != plan.len as usize || checksum(&state) != plan.sum {
            return self.fail(JoinFail::Aborted(Abort::Checksum), true);
        }
        self.state = JoinState::Restoring {
            plan: plan.clone(),
            since: now,
        };
        vec![
            JoinOut::Send(Msg::Ack { next }),
            JoinOut::Restore { plan, state },
        ]
    }

    /// The game side restored the state (`ok`) or could not.
    pub fn restored(&mut self, now: u64, ok: bool) -> Vec<JoinOut> {
        let JoinState::Restoring { plan, .. } = &self.state else {
            return vec![];
        };
        if !ok {
            return self.fail(JoinFail::Aborted(Abort::Restore), true);
        }
        let plan = plan.clone();
        let out = vec![
            JoinOut::Send(Msg::Ready { tick: plan.tick }),
            JoinOut::Joined {
                slot: plan.slot,
                tick: plan.tick.wrapping_add(1),
            },
        ];
        self.state = JoinState::Joined { plan, sent_at: now };
        out
    }

    /// The first sync update arrived: the host has the ready.
    pub fn streaming(&mut self) {
        if matches!(self.state, JoinState::Joined { .. }) {
            self.state = JoinState::Done;
        }
    }

    /// Resends and timeouts.
    pub fn poll(&mut self, now: u64) -> Vec<JoinOut> {
        match &mut self.state {
            JoinState::Asking { since, sent_at } => {
                if now.saturating_sub(*since) > ASK_MS {
                    return self.fail(JoinFail::NoAnswer, false);
                }
                if now.saturating_sub(*sent_at) >= RESEND_MS {
                    *sent_at = now;
                    return vec![JoinOut::Send(Msg::Request)];
                }
                vec![]
            }
            JoinState::Receiving { progress, .. } => {
                if now.saturating_sub(*progress) > STALL_MS {
                    return self.fail(JoinFail::Aborted(Abort::Timeout), true);
                }
                vec![]
            }
            JoinState::Restoring { since, .. } => {
                if now.saturating_sub(*since) > HOLD_MS {
                    return self.fail(JoinFail::Aborted(Abort::Timeout), true);
                }
                vec![]
            }
            JoinState::Joined { plan, sent_at } => {
                if now.saturating_sub(*sent_at) >= RESEND_MS {
                    *sent_at = now;
                    return vec![JoinOut::Send(Msg::Ready { tick: plan.tick })];
                }
                vec![]
            }
            JoinState::Done => vec![],
        }
    }

    fn fail(&mut self, why: JoinFail, tell: bool) -> Vec<JoinOut> {
        self.state = JoinState::Done;
        let mut out = Vec::new();
        if tell && let JoinFail::Aborted(a) = why {
            out.push(JoinOut::Send(Msg::Abort(a)));
        }
        out.push(JoinOut::BackToLobby(why));
        out
    }
}

/// The newcomer's log line once it runs (the co-op test's verdict looks
/// for it).
pub fn joined_line(slot: u8, tick: u32) -> String {
    format!(
        "coop-jip: joined in progress at tick {tick} as player {}",
        u32::from(slot) + 1
    )
}

/// The host's log line once it admits a player.
pub fn admitted_line(slot: u8, tick: u32, bytes: u32, held_ms: u64) -> String {
    format!(
        "coop-jip: admitted player {} at tick {tick} ({} KiB sent, held {held_ms} ms)",
        u32::from(slot) + 1,
        bytes.div_ceil(1024)
    )
}

#[cfg(test)]
mod tests;
