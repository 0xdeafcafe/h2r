//! Network calls, randomness and XAPI reads share one call index so a change
//! in their interleaving is a divergence even if each result still matches.
use crate::Tag;
use serde::{Deserialize, Serialize};

mod codec;
pub use codec::{decode_batch, encode_batch};

/// The chunk tag of a call batch: the binary stream of
/// [`codec`](encode_batch), every stream interleaved in call order.
pub const TAPE: Tag = *b"TAPE";
pub const TAPE_PART: Tag = *b"TPAR";
const PART_BYTES: usize = 1 << 20;
const PARTS: u8 = 0xC8;

/// A fragmented call ends with a TAPE record giving its index and the
/// number of immediately preceding TPAR records.
pub(crate) fn fragments(raw: &[u8]) -> FilmResult<Option<(u64, usize)>> {
    if raw.first() != Some(&PARTS) {
        return Ok(None);
    }
    ensure!(raw.len() == 17, "invalid tape fragment descriptor");
    let index = u64::from_le_bytes(raw[1..9].try_into()?);
    let parts = usize::try_from(u64::from_le_bytes(raw[9..].try_into()?))?;
    ensure!(parts > 0, "empty fragmented call");
    Ok(Some((index, parts)))
}

pub fn read_batch(reader: &mut Reader<File>, entry: &Entry, raw: &[u8]) -> FilmResult<Vec<Call>> {
    let Some((index, count)) = fragments(raw)? else { return decode_batch(raw) };
    let end = reader.entries.binary_search_by_key(&entry.offset, |e| e.offset)
        .map_err(|_| anyhow::anyhow!("missing tape entry"))?;
    let start = end.checked_sub(count).context("missing tape fragments")?;
    let mut data = Vec::new();
    for pos in start..end {
        let part = reader.entries[pos].clone();
        ensure!(part.tag == TAPE_PART, "missing tape fragment");
        data.extend(reader.read_bytes_limited(&part, PART_BYTES)?);
    }
    let calls = decode_batch(&data)?;
    ensure!(calls.len() == 1 && calls[0].index == index, "invalid fragmented tape call");
    Ok(calls)
}

/// XAPI reads share the HLE call cursor; topology changes are sparse and
/// indexed by the XAPI pump (the guest also sees its device-change globals).
pub const PADR: Tag = *b"PADR";
/// Retail virtual-keyboard commands, interleaved with other guest calls.
pub const TEXT: Tag = *b"TEXT";
/// Private retail phantom builder outcomes, on the shared call cursor.
pub const BTAP: Tag = *b"BTAP";
/// Sparse changes to the host's bot membership and controller-port assignments.
pub const BOTR: Tag = *b"BOTR";
/// Physical drawable observations made by the guest Video screen.
pub const VSIZ: Tag = *b"VSIZ";
pub const VIDEO_MENU_SIZE: &str = "Video.MenuSize";
pub const PDEV: Tag = *b"PDEV";

/// Streams of indexed HLE calls, in report order. A stream is a way to
/// count calls ([`stream_of`]); the film stores them interleaved in
/// [`TAPE`] batches. PDEV and BOTR carry separate boundary records and do
/// not advance the call cursor.
pub const CALL_STREAMS: [Tag; 6] = [crate::NETR, crate::RAND, PADR, TEXT, BTAP, VSIZ];

/// Whether any call on the tape is `op`. Reads every batch.
pub fn holds_op(reader: &mut Reader<File>, op: &str) -> FilmResult<bool> {
    let entries: Vec<Entry> = reader
        .machine_section()
        .entries()
        .filter(|e| e.tag == TAPE)
        .cloned()
        .collect();
    for entry in entries {
        let raw = reader.read_bytes(&entry)?;
        if read_batch(reader, &entry, &raw)?.iter().any(|c| c.op == op) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The stream an operation belongs to.
pub fn stream_of(op: &str) -> Tag {
    if op == VIDEO_MENU_SIZE {
        VSIZ
    } else if op == "Bot.BuildAction" {
        BTAP
    } else if op == "VirtualKeyboard" {
        TEXT
    } else if op == "XInputGetState" {
        PADR
    } else if matches!(op, "Random" | "CreateKey") || op.starts_with("Entropy.") {
        crate::RAND
    } else {
        crate::NETR
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortChange {
    /// 0 describes the state before the first pump.
    pub sync: u64,
    /// Each occupied port holds (generation, rumble supported).
    pub ports: [Option<(u32, bool)>; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub index: u64,
    pub op: String,
    pub args: Vec<u32>,
    pub thread: u32,
    pub result: Result<u32, u32>,
    /// The operation defines each output slot, including the XNQOS allocation.
    pub outputs: Vec<Vec<u8>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub tick: u64,
    pub sim: Option<u64>,
    pub calls: u64,
}

/// One tap per HLE instance. Recording failures are diagnostic only. During
/// replay even an exhausted stream stays disconnected.
pub trait Tap: Send + Sync {
    fn replaying(&self) -> bool;
    /// v2 films tape the full guest-facing asynchronous service ABI.
    fn service_calls(&self) -> bool { false }
    fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call>;
    fn record(&self, call: Call);
    fn fault(&self, reason: String);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mark {
    Lobby {
        boundary: u64,
        method: String,
        params: serde_json::Value,
    },
    End {
        slices: u64,
        tick: u64,
        calls: u64,
    },
    D3dBarrier {
        slices: u64,
        tick: u64,
        saved: bool,
    },
    Bookmark {
        tick: u64,
        title: String,
    },
}

/// A recorded D3D barrier, numbered from one in the original tape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturePoint {
    pub ordinal: u64,
    pub offset: u64,
    pub slices: u64,
    pub tick: u64,
    pub saved: bool,
}

use crate::{Entry, Reader, Writer};
use anyhow::{Context, Result as FilmResult, ensure};
use std::{collections::VecDeque, fs::File};

/// Flush before a checkpoint, an arena hash, or 128 calls / 64 KiB. A
/// change of stream does not flush: pad reads and socket calls share one
/// batch in call order.
#[derive(Default)]
pub struct Batch {
    pending: Vec<Call>,
    bytes: usize,
}
impl Batch {
    pub fn append(&mut self, out: &mut Writer<File>, call: Call) -> FilmResult<()> {
        // Validate before queuing: an unencodable call must never poison
        // the buffered prefix when teardown comes back to cut the film.
        let storage = codec::call_storage(&call)?;
        if self.pending.len() >= 128 || self.bytes + storage >= 65536 {
            self.flush(out)?;
        }
        if storage >= 65536 {
            let bytes = encode_batch(std::slice::from_ref(&call))?;
            if bytes.len() <= PART_BYTES {
                return out.chunk_bytes(TAPE, &bytes);
            }
            let parts = bytes.chunks(PART_BYTES);
            let mut end = vec![PARTS];
            end.extend(call.index.to_le_bytes());
            end.extend((parts.len() as u64).to_le_bytes());
            for part in parts {
                out.chunk_bytes(TAPE_PART, part)?;
            }
            return out.chunk_bytes(TAPE, &end);
        }
        self.bytes += storage;
        self.pending.push(call);
        Ok(())
    }
    pub fn flush(&mut self, out: &mut Writer<File>) -> FilmResult<()> {
        if !self.pending.is_empty() {
            out.chunk_bytes(TAPE, &encode_batch(&self.pending)?)?;
            self.pending.clear();
            self.bytes = 0;
        }
        Ok(())
    }
}

/// Sparse film rows are retained once per player. A ring entry records only
/// the index of the first unread row, even when the next call is mid-batch.
pub struct TapeRows<T> {
    rows: Vec<T>,
    next: usize,
}

impl<T> TapeRows<T> {
    fn new(rows: Vec<T>) -> Self {
        Self { rows, next: 0 }
    }

    pub fn front(&self) -> Option<&T> {
        self.rows.get(self.next)
    }

    pub fn is_empty(&self) -> bool {
        self.next == self.rows.len()
    }

    pub fn pop_front(&mut self) -> Option<T> where T: Clone {
        let row = self.front()?.clone();
        self.next += 1;
        Some(row)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackCursor {
    streams: usize,
    batch_left: usize,
    barriers: usize,
    checks: usize,
    lobby: usize,
}

/// Only the current call batch is inflated. Hashes and marks are sparse.
pub struct Playback {
    pub reader: Reader<File>,
    streams: TapeRows<Entry>,
    batch: VecDeque<Call>,
    barriers: TapeRows<CapturePoint>,
    pub checks: TapeRows<Check>,
    pub lobby: TapeRows<(u64, String, serde_json::Value)>,
    pub end: Option<(u64, u64, u64)>,
    pub length: u64,
}
impl Playback {
    /// A playback with no tape: a film played from its game start has no
    /// calls, hash checks, capture boundaries or lobby marks, and its
    /// length is the film file's (`FilmActions::length_ticks`). The reader
    /// is held for the film's header only; no section is read.
    pub fn empty(reader: Reader<File>, length: u64) -> Self {
        Self {
            reader,
            streams: TapeRows::new(Vec::new()),
            batch: VecDeque::new(),
            barriers: TapeRows::new(Vec::new()),
            checks: TapeRows::new(Vec::new()),
            lobby: TapeRows::new(Vec::new()),
            end: None,
            length,
        }
    }

    pub fn load(mut reader: Reader<File>) -> FilmResult<Self> {
        let mut streams = Vec::new();
        let mut checks = Vec::new();
        let mut barriers = Vec::new();
        let mut lobby = Vec::new();
        let mut end = None;
        for e in reader.machine_section().entries().cloned().collect::<Vec<_>>() {
            ensure!(
                end.is_none() || e.tag == crate::HEAD,
                "unexpected data after film end"
            );
            match e.tag {
                TAPE => streams.push(e),
                crate::HASH => {
                    let check: Check = reader.read(&e)?;
                    ensure!(
                        check.tick == (checks.len() as u64 + 1) * 30,
                        "missing film checkpoint"
                    );
                    checks.push(check);
                }
                crate::MARK => match reader.read(&e)? {
                    Mark::Lobby {
                        boundary,
                        method,
                        params,
                    } => {
                        ensure!(lobby.last().is_none_or(|(last, _, _)| *last <= boundary),
                            "out of order film lobby boundary");
                        lobby.push((boundary, method, params));
                    }
                    Mark::End {
                        slices,
                        tick,
                        calls,
                    } => end = Some((slices, tick, calls)),
                    Mark::D3dBarrier {
                        slices,
                        tick,
                        saved,
                    } => {
                        ensure!(
                            reader.header.version >= 2
                                && barriers
                                    .last()
                                    .is_none_or(|last: &CapturePoint| slices > last.slices
                                        && tick >= last.tick),
                            "invalid D3D capture boundary"
                        );
                        barriers.push(CapturePoint {
                            ordinal: barriers.len() as u64 + 1,
                            offset: e.offset,
                            slices,
                            tick,
                            saved,
                        });
                    }
                    Mark::Bookmark { .. } => {}
                },
                _ => {}
            }
        }
        let legacy = reader.header.version == 1;
        if legacy {
            ensure!(!checks.is_empty(), "film has no complete hash interval");
            ensure!(
                checks.iter().any(|c| c.sim.is_some()),
                "film has no simulation hashes"
            );
        }
        let last = checks.last().map_or(0, |check| check.tick);
        if let Some((_, tick, calls)) = end {
            ensure!(
                tick / 30 == last / 30
                    && tick == reader.header.meta.length_ticks
                    && checks.last().is_none_or(|check| calls >= check.calls),
                "inconsistent film end"
            );
        } else {
            ensure!(!reader.finalized && legacy, "film has no end");
        }
        let length = end.map_or(last, |(_, t, _)| t);
        Ok(Self {
            reader,
            streams: TapeRows::new(streams),
            batch: VecDeque::new(),
            barriers: TapeRows::new(barriers),
            checks: TapeRows::new(checks),
            lobby: TapeRows::new(lobby),
            end,
            length,
        })
    }


    pub fn cursor(&self) -> PlaybackCursor {
        PlaybackCursor {
            streams: self.streams.next,
            batch_left: self.batch.len(),
            barriers: self.barriers.next,
            checks: self.checks.next,
            lobby: self.lobby.next,
        }
    }

    /// Restore an in-memory cursor. Only a batch which straddles the capture
    /// boundary needs to be read again.
    pub fn resume_cursor(&mut self, cursor: PlaybackCursor) -> FilmResult<()> {
        ensure!(cursor.streams <= self.streams.rows.len()
            && cursor.barriers <= self.barriers.rows.len()
            && cursor.checks <= self.checks.rows.len()
            && cursor.lobby <= self.lobby.rows.len(), "invalid film tape cursor");
        let mut batch = VecDeque::new();
        if cursor.batch_left != 0 {
            let index = cursor.streams.checked_sub(1).context("missing current film call batch")?;
            batch = Self::read_call_batch(&mut self.reader, &self.streams.rows[index])?;
            let skip = batch.len().checked_sub(cursor.batch_left).context("short film call batch")?;
            batch.drain(..skip);
        }
        self.batch = batch;
        self.streams.next = cursor.streams;
        self.barriers.next = cursor.barriers;
        self.checks.next = cursor.checks;
        self.lobby.next = cursor.lobby;
        Ok(())
    }

    /// All recorded capture points, including boundaries behind the current cursor.
    pub fn capture_points(&self) -> Vec<u64> {
        self.barriers.rows.iter().map(|point| point.tick).collect()
    }

    /// A linear run repeats D3D's push-buffer drain, target writeback and
    /// visibility queries at the same scheduler boundary as the recorder.
    pub fn d3d_barrier(&mut self, slices: u64, tick: u64) -> FilmResult<Option<bool>> {
        let Some(&point) = self.barriers.front() else {
            return Ok(None);
        };
        let CapturePoint {
            slices: at,
            tick: expected_tick,
            saved,
            ..
        } = point;
        if slices < at {
            return Ok(None);
        }
        ensure!(slices == at && tick == expected_tick,
            "first divergence at tick {tick}: expected D3D capture at slice {at}, tick {expected_tick}");
        self.barriers.pop_front();
        Ok(Some(saved))
    }

    pub fn call(&mut self, index: u64, op: &str, args: &[u32], thread: u32) -> FilmResult<Call> {
        if self.batch.is_empty() {
            let entry = self.streams.pop_front().context("past end of film calls")?;
            self.batch = Self::read_call_batch(&mut self.reader, &entry)?;
        }
        let c = self.batch.pop_front().context("empty film call batch")?;
        ensure!(
            c.index == index && c.op == op && c.args == args && c.thread == thread,
            "expected {} call {} thread {}, saw {op} call {index} thread {thread}",
            c.op,
            c.index,
            c.thread
        );
        Ok(c)
    }

    fn read_call_batch(reader: &mut Reader<File>, entry: &Entry) -> FilmResult<VecDeque<Call>> {
        let raw = reader.read_bytes(entry)?;
        let batch = read_batch(reader, entry, &raw)?;
        ensure!(batch.len() <= 128, "invalid film call batch");
        Ok(batch.into())
    }

    pub fn exhausted(&self) -> bool {
        self.streams.is_empty()
            && self.barriers.is_empty()
            && self.batch.is_empty()
            && self.checks.is_empty()
            && self.lobby.is_empty()
    }
}
