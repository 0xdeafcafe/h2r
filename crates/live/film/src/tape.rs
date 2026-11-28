//! A client film's machine tape (docs/game/film-format.md, "The machine
//! tape"; docs/game/theater-research.md, "What the recorder captures").
//!
//! A client of a System Link or Live game is sent state, not every
//! player's updates, so its game is not replayed from the simulation's
//! inputs. It is replayed as a machine: everything that reached the guest
//! from outside its own computation is on the tape, keyed by call index at
//! its HLE boundary, and the replay hands call *n* of the tape back as call
//! *n* of the run. Two streams:
//!
//! - **Calls**: every Live HLE entry the guest made (`live_hle::tape`):
//!   socket receives (`recv`, `recvfrom`: payload, source address, error),
//!   `select` and FIONREAD results, `XNetGetConnectStatus`, QoS results,
//!   host randomness and the Live service's answers. Each holds what the
//!   call returned and every guest byte it wrote.
//! - **Pads**: every `XInputGetState` result (`xapi::tap`).
//!
//! A tape block is one stream's run of consecutive entries. Inflated, it
//! is `u8 version | u8 stream | u64 first index | u32 count` and then the
//! entries; the film file stores it as a block of kind 6 in one zstd frame
//! (`film::file`). Numbers are ULEB128 unless named.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// The tape layout this build writes and reads.
pub const VERSION: u8 = 1;
/// Bytes of one `XINPUT_STATE` (packet number and gamepad).
pub const PACKET: usize = 22;
/// The most entries one block holds.
pub const BLOCK_ENTRIES: usize = 4096;
/// The most bytes one block holds, inflated.
pub const MAX_BLOCK_BYTES: usize = 8 << 20;
/// The most argument words a call holds (the widest Live entry takes 12).
pub const MAX_ARGS: usize = 16;

/// Whose game a film is, as the recording machine played it
/// (docs/game/film-format.md, "The machine tape"). A film without one is `Local`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Campaign, split screen, bots, co-op: this machine simulates.
    #[default]
    Local,
    /// This machine hosts the System Link or Live game.
    Host,
    /// A client of a System Link game another machine hosts.
    SystemLinkClient,
    /// A client of a Live game another machine hosts.
    LiveClient,
}

impl Role {
    /// A client film replays as a machine, from its tape.
    pub fn is_client(self) -> bool {
        matches!(self, Role::SystemLinkClient | Role::LiveClient)
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Local => "local",
            Role::Host => "host",
            Role::SystemLinkClient => "system link client",
            Role::LiveClient => "Live client",
        }
    }

    pub fn is_local(&self) -> bool {
        *self == Role::Local
    }
}

/// One stream of a tape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Calls = 0,
    Pads = 1,
}

impl Stream {
    fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => Stream::Calls,
            1 => Stream::Pads,
            _ => bail!("tape block of unknown stream {b}"),
        })
    }
}

/// One Live HLE call: the entry's guest address, its stack arguments and
/// what it gave the guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub entry: u32,
    pub args: Vec<u32>,
    pub answer: Answer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The call returned to the guest.
    Return(Returned),
    /// The call stopped the guest (an unimplemented or unverified entry,
    /// or a guest fault), with what it said.
    Stop(String),
}

/// What a call that returned gave the guest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Returned {
    /// `eax`.
    pub value: u32,
    /// The calling thread's `WSAGetLastError` after the call, when the call
    /// changed it.
    pub error: Option<u32>,
    /// Storage the call mapped zeroed and claimed for the HLE: (address,
    /// bytes, permission bits 1 read, 2 write, 4 execute).
    pub zeroed: Vec<(u32, u32, u8)>,
    /// Every guest byte the call wrote, in order.
    pub writes: Vec<(u32, Vec<u8>)>,
    /// A QoS result's allocation in the HLE's QoS storage: (bytes asked,
    /// address given). The replay makes the same allocation, so a later
    /// `XNetQosRelease` frees it.
    pub alloc: Option<(u32, u32)>,
}

/// One `XInputGetState` result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadRead {
    pub handle: u32,
    pub status: u32,
    pub packet: [u8; PACKET],
}

/// A whole tape: both streams from index 0.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tape {
    pub calls: Vec<Call>,
    pub pads: Vec<PadRead>,
}

impl Tape {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.pads.is_empty()
    }

    /// The tape as blocks, each at most [`BLOCK_ENTRIES`] entries and
    /// [`MAX_BLOCK_BYTES`] bytes: calls first, then pads.
    pub fn blocks(&self) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut first = 0usize;
        while first < self.calls.len() {
            let (block, taken) = encode_calls(first as u64, &self.calls[first..])?;
            out.push(block);
            first += taken;
        }
        let mut first = 0usize;
        while first < self.pads.len() {
            let end = (first + BLOCK_ENTRIES).min(self.pads.len());
            out.push(encode_pads(first as u64, &self.pads[first..end]));
            first = end;
        }
        Ok(out)
    }

    /// Add one decoded block. Each stream's blocks come in order with no
    /// gap or overlap.
    pub fn push_block(&mut self, bytes: &[u8]) -> Result<()> {
        match decode_block(bytes)? {
            Block::Calls { first, calls } => {
                ensure!(
                    first == self.calls.len() as u64,
                    "tape call block starts at call {first}, after {} calls",
                    self.calls.len()
                );
                self.calls.extend(calls);
            }
            Block::Pads { first, pads } => {
                ensure!(
                    first == self.pads.len() as u64,
                    "tape pad block starts at read {first}, after {} reads",
                    self.pads.len()
                );
                self.pads.extend(pads);
            }
        }
        Ok(())
    }
}

/// One decoded block.
#[derive(Debug, PartialEq, Eq)]
pub enum Block {
    Calls { first: u64, calls: Vec<Call> },
    Pads { first: u64, pads: Vec<PadRead> },
}

fn put(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn head(stream: Stream, first: u64, count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(14);
    out.push(VERSION);
    out.push(stream as u8);
    out.extend_from_slice(&first.to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    out
}

fn encode_call(out: &mut Vec<u8>, call: &Call) -> Result<()> {
    ensure!(call.args.len() <= MAX_ARGS, "a tape call holds at most {MAX_ARGS} arguments");
    put(out, call.entry.into());
    out.push(call.args.len() as u8);
    for &a in &call.args {
        put(out, a.into());
    }
    match &call.answer {
        Answer::Return(r) => {
            out.push(0);
            put(out, r.value.into());
            match r.error {
                Some(e) => {
                    out.push(1);
                    put(out, e.into());
                }
                None => out.push(0),
            }
            put(out, r.zeroed.len() as u64);
            for &(at, len, permissions) in &r.zeroed {
                put(out, at.into());
                put(out, len.into());
                out.push(permissions);
            }
            put(out, r.writes.len() as u64);
            for (at, bytes) in &r.writes {
                put(out, (*at).into());
                put(out, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
            match r.alloc {
                Some((size, at)) => {
                    out.push(1);
                    put(out, size.into());
                    put(out, at.into());
                }
                None => out.push(0),
            }
        }
        Answer::Stop(why) => {
            out.push(1);
            put(out, why.len() as u64);
            out.extend_from_slice(why.as_bytes());
        }
    }
    Ok(())
}

/// A block of the calls from `first`: as many of `calls` as fit, and how
/// many that was (at least one; a call larger than a block is an error).
pub fn encode_calls(first: u64, calls: &[Call]) -> Result<(Vec<u8>, usize)> {
    let mut body = Vec::new();
    let mut taken = 0;
    for call in calls.iter().take(BLOCK_ENTRIES) {
        let before = body.len();
        encode_call(&mut body, call)?;
        if body.len() + 14 > MAX_BLOCK_BYTES {
            ensure!(taken > 0, "tape call {} is larger than a tape block", first);
            body.truncate(before);
            break;
        }
        taken += 1;
    }
    let mut out = head(Stream::Calls, first, taken);
    out.extend_from_slice(&body);
    Ok((out, taken))
}

/// A block of the pad reads from `first`.
pub fn encode_pads(first: u64, pads: &[PadRead]) -> Vec<u8> {
    let mut out = head(Stream::Pads, first, pads.len());
    for p in pads {
        put(&mut out, p.handle.into());
        put(&mut out, p.status.into());
        out.extend_from_slice(&p.packet);
    }
    out
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len());
        let end = end.context("tape block is cut short")?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn number(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            value |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("tape block number is too long")
    }
    fn word(&mut self) -> Result<u32> {
        u32::try_from(self.number()?).context("tape block number is past 32 bits")
    }
    fn len(&mut self, limit: usize) -> Result<usize> {
        let n = self.number()?;
        ensure!(n <= limit as u64, "tape block length {n} is past {limit}");
        Ok(n as usize)
    }
}

fn decode_call(c: &mut Cursor<'_>) -> Result<Call> {
    let entry = c.word()?;
    let nargs = usize::from(c.byte()?);
    ensure!(nargs <= MAX_ARGS, "tape call holds {nargs} arguments");
    let args = (0..nargs).map(|_| c.word()).collect::<Result<Vec<_>>>()?;
    let answer = match c.byte()? {
        0 => {
            let value = c.word()?;
            let error = match c.byte()? {
                0 => None,
                1 => Some(c.word()?),
                b => bail!("tape call has a bad error flag {b}"),
            };
            let zeroed = (0..c.len(MAX_BLOCK_BYTES)?)
                .map(|_| -> Result<_> { Ok((c.word()?, c.word()?, c.byte()?)) })
                .collect::<Result<Vec<_>>>()?;
            let writes = (0..c.len(MAX_BLOCK_BYTES)?)
                .map(|_| -> Result<_> {
                    let at = c.word()?;
                    let n = c.len(MAX_BLOCK_BYTES)?;
                    Ok((at, c.take(n)?.to_vec()))
                })
                .collect::<Result<Vec<_>>>()?;
            let alloc = match c.byte()? {
                0 => None,
                1 => Some((c.word()?, c.word()?)),
                b => bail!("tape call has a bad allocation flag {b}"),
            };
            Answer::Return(Returned { value, error, zeroed, writes, alloc })
        }
        1 => {
            let n = c.len(MAX_BLOCK_BYTES)?;
            let why = std::str::from_utf8(c.take(n)?).context("tape stop is not UTF-8")?;
            Answer::Stop(why.to_owned())
        }
        b => bail!("tape call has a bad answer kind {b}"),
    };
    Ok(Call { entry, args, answer })
}

/// Read one block. A block of a newer tape version is named, not guessed.
pub fn decode_block(bytes: &[u8]) -> Result<Block> {
    ensure!(bytes.len() <= MAX_BLOCK_BYTES, "tape block is larger than {MAX_BLOCK_BYTES} bytes");
    let mut c = Cursor { bytes, at: 0 };
    let version = c.byte()?;
    ensure!(
        version == VERSION,
        "tape block version {version}; this build reads version {VERSION}"
    );
    let stream = Stream::from_byte(c.byte()?)?;
    let first = u64::from_le_bytes(c.take(8)?.try_into()?);
    let count = u32::from_le_bytes(c.take(4)?.try_into()?) as usize;
    ensure!(count <= BLOCK_ENTRIES, "tape block holds {count} entries");
    let block = match stream {
        Stream::Calls => Block::Calls {
            first,
            calls: (0..count).map(|_| decode_call(&mut c)).collect::<Result<_>>()?,
        },
        Stream::Pads => Block::Pads {
            first,
            pads: (0..count)
                .map(|_| -> Result<_> {
                    Ok(PadRead {
                        handle: c.word()?,
                        status: c.word()?,
                        packet: c.take(PACKET)?.try_into()?,
                    })
                })
                .collect::<Result<_>>()?,
        },
    };
    ensure!(c.at == bytes.len(), "tape block has bytes after its last entry");
    Ok(block)
}

#[cfg(test)]
mod tests;
