//! The two HLE streams share one call index so moving a random draw across
//! a socket read is a divergence even if both streams' values still match.
use crate::Tag;
use serde::{Deserialize, Serialize};

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
    pub checks: VecDeque<Check>,
    pub lobby: VecDeque<(u64, String, serde_json::Value)>,
    pub end: Option<(u64, u64, u64)>,
    pub length: u64,
}
impl Playback {
    pub fn load(mut reader: Reader<File>) -> FilmResult<Self> {
        let mut streams = VecDeque::new();
        let mut checks = VecDeque::new();
        let mut lobby = VecDeque::new();
        let mut end = None;
        let mut data = 0;
        for e in reader.entries.clone() {
            ensure!(
                end.is_none() || e.tag == crate::HEAD,
                "unexpected data after film end"
            );
            match e.tag {
                crate::NETR | crate::RAND => streams.push_back(e),
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
                    Mark::Bookmark { .. } => {}
                },
                _ => {}
            }
        }
        ensure!(data == 1, "film requires one start manifest");
        let last = checks
            .back()
            .context("film has no complete hash interval")?
            .tick;
        ensure!(
            checks.iter().any(|c| c.sim.is_some()),
            "film has no simulation hashes"
        );
        if let Some((_, tick, calls)) = end {
            ensure!(
                tick / 30 == last / 30
                    && tick == reader.header.meta.length_ticks
                    && calls >= checks.back().unwrap().calls,
                "inconsistent film end"
            );
        } else {
            ensure!(!reader.finalized, "finalized film has no end");
        }
        let length = end.map_or(last, |(_, t, _)| t);
        Ok(Self {
            reader,
            streams,
            batch: VecDeque::new(),
            checks,
            lobby,
            end,
            length,
        })
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
                    .all(|c| (matches!(c.op.as_str(), "Random" | "CreateKey")
                        || c.op.starts_with("Entropy."))
                        == (entry.tag == crate::RAND)),
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
            && self.batch.is_empty()
            && self.checks.is_empty()
            && self.lobby.is_empty()
    }
}
