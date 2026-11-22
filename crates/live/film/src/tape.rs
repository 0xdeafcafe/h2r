//! Network calls, randomness and XAPI reads share one call index so a change
//! in their interleaving is a divergence even if each result still matches.
use crate::Tag;
use serde::{Deserialize, Serialize};

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

/// Streams of indexed HLE calls, in report order. PDEV and BOTR carry
/// separate boundary records and do not advance the call cursor.
pub const CALL_STREAMS: [Tag; 6] = [crate::NETR, crate::RAND, PADR, TEXT, BTAP, VSIZ];

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
    fn record(&self, tag: Tag, call: Call);
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

/// Cursor for the machine tape inside the film, independent of the location
/// of the KEYF envelope inside the sidecar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidecarCursor {
    pub mark_offset: u64,
}

impl SidecarCursor {
    pub const fn new(mark_offset: u64) -> Self {
        Self { mark_offset }
    }
}

use crate::{Entry, Reader, Writer};
use anyhow::{Context, Result as FilmResult, ensure};
use std::{collections::VecDeque, fs::File};

/// Flush before a checkpoint, a stream change, or 128 calls / 64 KiB.
#[derive(Default)]
pub struct Batch {
    tag: Tag,
    pending: Vec<Call>,
    bytes: usize,
}
impl Batch {
    pub fn append(&mut self, out: &mut Writer<File>, tag: Tag, call: Call) -> FilmResult<()> {
        if self.tag != tag || self.pending.len() >= 128 || self.bytes >= 65536 {
            self.flush(out)?;
        }
        self.tag = tag;
        self.bytes += call.outputs.iter().map(Vec::len).sum::<usize>();
        self.pending.push(call);
        Ok(())
    }
    pub fn flush(&mut self, out: &mut Writer<File>) -> FilmResult<()> {
        if !self.pending.is_empty() {
            out.chunk(self.tag, &self.pending)?;
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
    last_barrier: Option<CapturePoint>,
    last_call_chunk: Option<u64>,
    pub checks: TapeRows<Check>,
    pub lobby: TapeRows<(u64, String, serde_json::Value)>,
    pub end: Option<(u64, u64, u64)>,
    pub length: u64,
}
impl Playback {
    pub fn load(mut reader: Reader<File>) -> FilmResult<Self> {
        let mut streams = Vec::new();
        let mut checks = Vec::new();
        let mut barriers = Vec::new();
        let mut lobby = Vec::new();
        let mut end = None;
        let mut data = 0;
        for e in reader.machine_section().entries().cloned().collect::<Vec<_>>() {
            ensure!(
                end.is_none() || e.tag == crate::HEAD,
                "unexpected data after film end"
            );
            match e.tag {
                tag if CALL_STREAMS.contains(&tag) => streams.push(e),
                crate::DATA => data += 1,
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
        ensure!(data == 1, "film requires one start manifest");
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
            last_barrier: None,
            last_call_chunk: None,
            checks: TapeRows::new(checks),
            lobby: TapeRows::new(lobby),
            end,
            length,
        })
    }
    /// Reset the call reader at the machine snapshot. The recorder
    /// flushes its pending call batch before each keyframe. No intervening
    /// network payload needs to be inflated to seek the call timeline.
    pub fn resume_keyframe(
        &mut self,
        offset: u64,
        tick: u64,
        calls: u64,
        lobby_boundary: u64,
    ) -> FilmResult<()> {
        self.reader.machine_section().keyframe_at(offset)?;
        self.resume_after(offset, tick, calls, lobby_boundary)
    }

    /// Resume at a checked in-film D3D barrier. Historic marks can precede
    /// the flush of already-consumed calls, so verify the next call index.
    pub fn resume_sidecar(
        &mut self,
        mark_offset: u64,
        frame: &crate::blam::Keyframe,
    ) -> FilmResult<()> {
        self.resume_sidecar_at(SidecarCursor::new(mark_offset), frame)
    }

    pub fn resume_sidecar_at(
        &mut self,
        cursor: SidecarCursor,
        frame: &crate::blam::Keyframe,
    ) -> FilmResult<()> {
        let mark_offset = cursor.mark_offset;
        ensure!(
            self.barriers.rows.iter().any(|point| point.offset == mark_offset
                && point.saved
                && point.tick == frame.tick
                && point.slices == frame.slices),
            "sidecar seek does not match a capture point"
        );
        let (_, _, end_calls) = self.end.context("sidecar seek requires a finished tape")?;
        let next = self
            .streams
            .rows
            .iter()
            .find(|e| e.offset > mark_offset)
            .cloned();
        let next_index = if let Some(entry) = next {
            Self::read_call_batch(&mut self.reader, &entry)?[0].index
        } else {
            end_calls
        };
        ensure!(
            next_index == frame.calls,
            "sidecar seek is not at a call batch boundary"
        );
        self.resume_after(mark_offset, frame.tick, frame.calls, frame.lobby_boundary)
    }

    fn resume_after(
        &mut self,
        offset: u64,
        tick: u64,
        calls: u64,
        lobby_boundary: u64,
    ) -> FilmResult<()> {
        ensure!(
            self.end.is_some_and(|(_, end_tick, end_calls)| tick <= end_tick && calls <= end_calls),
            "keyframe past film end"
        );
        self.batch.clear();
        self.streams.next = self.streams.rows.partition_point(|e| e.offset <= offset);
        self.barriers.next = self.barriers.rows.partition_point(|point| point.offset <= offset);
        self.checks.next = self.checks.rows.partition_point(|check| check.tick <= tick);
        self.lobby.next = self.lobby.rows.partition_point(|(at, _, _)| *at <= lobby_boundary);
        self.last_barrier = None;
        self.last_call_chunk = None;
        Ok(())
    }
    pub fn last_capture_point(&self) -> Option<CapturePoint> {
        self.last_barrier
    }

    /// The last mark is usable only when it cleanly splits the call tape.
    /// Legacy recorders could emit the mark before flushing pending calls.
    pub fn sidecar_capture_point(&self) -> Option<CapturePoint> {
        self.last_capture_point().filter(|point| {
            point.saved
                && self.batch.is_empty()
                && self
                    .last_call_chunk
                    .is_none_or(|offset| offset < point.offset)
                && self
                    .streams
                    .front()
                    .is_none_or(|entry| entry.offset > point.offset)
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
        self.last_barrier = cursor.barriers.checked_sub(1).map(|i| self.barriers.rows[i]);
        self.last_call_chunk = cursor.streams.checked_sub(1).map(|i| self.streams.rows[i].offset);
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
        self.last_barrier = Some(point);
        Ok(Some(saved))
    }

    pub fn call(&mut self, index: u64, op: &str, args: &[u32], thread: u32) -> FilmResult<Call> {
        if self.batch.is_empty() {
            let entry = self.streams.pop_front().context("past end of film calls")?;
            self.batch = Self::read_call_batch(&mut self.reader, &entry)?;
            self.last_call_chunk = Some(entry.offset);
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
        let batch: VecDeque<Call> = reader.read(entry)?;
        ensure!(
            !batch.is_empty() && batch.len() <= 128,
            "invalid film call batch"
        );
        ensure!(
            batch.iter().all(|call| {
                let tag = if call.op == VIDEO_MENU_SIZE {
                    VSIZ
                } else if call.op == "Bot.BuildAction" {
                    BTAP
                } else if call.op == "VirtualKeyboard" {
                    TEXT
                } else if call.op == "XInputGetState" {
                    PADR
                } else if matches!(call.op.as_str(), "Random" | "CreateKey")
                    || call.op.starts_with("Entropy.")
                {
                    crate::RAND
                } else {
                    crate::NETR
                };
                tag == entry.tag
            }),
            "wrong film stream"
        );
        Ok(batch)
    }

    pub fn exhausted(&self) -> bool {
        self.streams.is_empty()
            && self.barriers.is_empty()
            && self.batch.is_empty()
            && self.checks.is_empty()
            && self.lobby.is_empty()
    }
}
