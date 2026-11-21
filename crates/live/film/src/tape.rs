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

/// Only the current call batch is inflated. Hashes and marks are sparse.
pub struct Playback {
    pub reader: Reader<File>,
    streams: VecDeque<Entry>,
    batch: VecDeque<Call>,
    // File offset, scheduler slice, completed tick, save succeeded.
    barriers: VecDeque<(u64, u64, u64, bool)>,
    pub checks: VecDeque<Check>,
    pub lobby: VecDeque<(u64, String, serde_json::Value)>,
    pub end: Option<(u64, u64, u64)>,
    pub length: u64,
}
impl Playback {
    pub fn load(mut reader: Reader<File>) -> FilmResult<Self> {
        let mut streams = VecDeque::new();
        let mut checks = VecDeque::new();
        let mut barriers = VecDeque::new();
        let mut lobby = VecDeque::new();
        let mut end = None;
        let mut data = 0;
        for e in reader.machine_section().entries().cloned().collect::<Vec<_>>() {
            ensure!(
                end.is_none() || e.tag == crate::HEAD,
                "unexpected data after film end"
            );
            match e.tag {
                tag if CALL_STREAMS.contains(&tag) => streams.push_back(e),
                crate::DATA => data += 1,
                crate::HASH => {
                    let check: Check = reader.read(&e)?;
                    ensure!(
                        check.tick == (checks.len() as u64 + 1) * 30,
                        "missing film checkpoint"
                    );
                    checks.push_back(check);
                }
                crate::MARK => match reader.read(&e)? {
                    Mark::Lobby {
                        boundary,
                        method,
                        params,
                    } => lobby.push_back((boundary, method, params)),
                    Mark::End {
                        slices,
                        tick,
                        calls,
                    } => end = Some((slices, tick, calls)),
                    Mark::D3dBarrier { slices, tick, saved } => {
                        ensure!(reader.header.version >= 2 && barriers.back().is_none_or(
                            |&(_, last, last_tick, _)| slices > last && tick >= last_tick),
                            "invalid D3D capture boundary");
                        barriers.push_back((e.offset, slices, tick, saved));
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
        let last = checks.back().map_or(0, |check| check.tick);
        if let Some((_, tick, calls)) = end {
            ensure!(
                tick / 30 == last / 30
                    && tick == reader.header.meta.length_ticks
                    && checks.back().is_none_or(|check| calls >= check.calls),
                "inconsistent film end"
            );
        } else {
            ensure!(!reader.finalized && legacy, "film has no end");
        }
        let length = end.map_or(last, |(_, t, _)| t);
        Ok(Self {
            reader,
            streams,
            batch: VecDeque::new(),
            barriers,
            checks,
            lobby,
            end,
            length,
        })
    }
    /// Start a fresh call reader at the machine snapshot. The recorder
    /// flushes its pending call batch before each keyframe. No intervening
    /// network payload needs to be inflated to seek the call timeline.
    pub fn resume_keyframe(
        &mut self,
        offset: u64,
        tick: u64,
        calls: u64,
        lobby_boundary: u64,
    ) -> FilmResult<()> {
        ensure!(self.batch.is_empty(), "keyframe seek requires a fresh tape");
        self.reader.machine_section().keyframe_at(offset)?;
        ensure!(
            self.end.is_some_and(|(_, end_tick, end_calls)| tick <= end_tick && calls <= end_calls),
            "keyframe past film end"
        );
        self.streams.retain(|e| e.offset > offset);
        self.barriers.retain(|&(at, _, _, _)| at > offset);
        self.checks.retain(|check| check.tick > tick);
        self.lobby.retain(|(at, _, _)| *at > lobby_boundary);
        Ok(())
    }
    /// The recorded D3D capture boundaries still ahead of the tape cursor,
    /// as (file offset, scheduler slice, completed tick, save succeeded).
    pub fn barriers(&self) -> impl Iterator<Item = (u64, u64, u64, bool)> + '_ {
        self.barriers.iter().copied()
    }
    /// A linear run repeats D3D's push-buffer drain, target writeback and
    /// visibility queries at the same scheduler boundary as the recorder.
    pub fn d3d_barrier(&mut self, slices: u64, tick: u64) -> FilmResult<Option<bool>> {
        let Some(&(_, at, expected_tick, saved)) = self.barriers.front() else {
            return Ok(None);
        };
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
            let batch: VecDeque<Call> = self.reader.read(&entry)?;
            ensure!(
                !batch.is_empty() && batch.len() <= 128,
                "invalid film call batch"
            );
            ensure!(
                batch
                    .iter()
                    .all(|c| {
                        let tag = if c.op == VIDEO_MENU_SIZE {
                            VSIZ
                        } else if c.op == "Bot.BuildAction" {
                            BTAP
                        } else if c.op == "VirtualKeyboard" {
                            TEXT
                        } else if c.op == "XInputGetState" {
                            PADR
                        } else if matches!(c.op.as_str(), "Random" | "CreateKey")
                            || c.op.starts_with("Entropy.")
                        {
                            crate::RAND
                        } else {
                            crate::NETR
                        };
                        tag == entry.tag
                    }),
                "wrong film stream"
            );
            self.batch = batch;
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
    pub fn exhausted(&self) -> bool {
        self.streams.is_empty()
            && self.barriers.is_empty()
            && self.batch.is_empty()
            && self.checks.is_empty()
            && self.lobby.is_empty()
    }
}
