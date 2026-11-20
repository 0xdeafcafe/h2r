//! Validate the directory against physical chunk boundaries as the bytes pass
//! into storage. The public HTTP range reader trusts only that validated index.
use std::{fmt, io::Read};

use live_proto::film::{
    MAX_SUMMARY_BYTES, Summary,
    range::{self, Directory, Footer, Front, Prefix},
};
use serde::{
    Deserialize, Deserializer,
    de::{SeqAccess, Visitor},
};
use sha2::{Digest, Sha256};

use super::{ApiResult, import::FilmEvents, invalid, ranges::WebRanges};

use range::{MAX_INDEX_BYTES, MAX_INDEX_PACKED, MAX_TICK_BYTES, MAX_TICK_PACKED};
const MAX_CHUNK: usize = 80 << 20;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Deserialize)]
struct Entry {
    tag: [u8; 4],
    offset: u64,
    size: u32,
    #[serde(default)]
    machine_span: Option<u32>,
}

impl Entry {
    fn update(&self, digest: &mut Sha256) {
        digest.update(self.tag);
        digest.update(self.offset.to_le_bytes());
        digest.update(self.size.to_le_bytes());
    }
}

struct IndexDigest;
impl<'de> Visitor<'de> for IndexDigest {
    type Value = Sha256;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a film index")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut entries: A) -> Result<Self::Value, A::Error> {
        let mut digest = Sha256::new();
        let mut machine = live_proto::film::machine_index::MachineRuns::default();
        while let Some(entry) = entries.next_element::<Entry>()? {
            machine
                .feed(entry.tag, entry.machine_span)
                .map_err(serde::de::Error::custom)?;
            entry.update(&mut digest);
        }
        machine.finish().map_err(serde::de::Error::custom)?;
        Ok(digest)
    }
}

pub(super) struct InspectedFilm {
    pub(super) summary: Option<Summary>,
    pub(super) web_ranges: Option<WebRanges>,
    pub(super) events: FilmEvents,
}

struct CheckedDirectory {
    directory: Directory,
    offset: u64,
    packed_size: u32,
    raw_size: u32,
    sha256: String,
}

#[derive(Default)]
pub(super) struct Inspector {
    pos: u64,
    prefix: Vec<u8>,
    tag: [u8; 4],
    packed: u32,
    remaining: usize,
    collected: Vec<u8>,
    version: Option<u32>,
    front_buf: Option<Vec<u8>>,
    front_size: Option<u32>,
    final_head: Option<u64>,
    last_head: Option<u64>,
    observed: Sha256,
    ticks: Sha256,
    scene_starts: Vec<Option<bool>>,
    keys: Sha256,
    chunks: usize,
    summary: Option<Summary>,
    cidx: Option<CheckedDirectory>,
    index: Option<u64>,
    footer: Vec<u8>,
    pub(super) events: FilmEvents,
}

fn digest_tick(
    d: &mut Sha256,
    first: u64,
    last: u64,
    offset: u64,
    packed: u32,
    raw: u32,
    clock: &[(u8, Option<u64>)],
) {
    d.update(first.to_le_bytes());
    d.update(last.to_le_bytes());
    d.update(offset.to_le_bytes());
    d.update(packed.to_le_bytes());
    d.update(raw.to_le_bytes());
    d.update((clock.len() as u64).to_le_bytes());
    for (offset, guest) in clock {
        d.update([*offset, u8::from(guest.is_some())]);
        d.update(guest.unwrap_or(0).to_le_bytes());
    }
}

impl Inspector {
    #[cfg(test)]
    pub(super) fn consume(&mut self, mut bytes: &[u8]) -> ApiResult<()> {
        while !bytes.is_empty() {
            let n = self.consume_one(bytes)?;
            bytes = &bytes[n..];
        }
        Ok(())
    }

    /// Stop at the first complete TICK so the importer can drain its rows
    /// before expanding another compressed chunk from the same HTTP buffer.
    pub(super) fn consume_one(&mut self, mut bytes: &[u8]) -> ApiResult<usize> {
        let input = bytes.len();
        while !bytes.is_empty() {
            if let Some(front) = self.front_buf.as_mut() {
                let n = bytes.len().min(range::DATA_OFFSET as usize - front.len());
                front.extend_from_slice(&bytes[..n]);
                self.pos += n as u64;
                bytes = &bytes[n..];
                if self.pos == range::DATA_OFFSET {
                    let buf = self.front_buf.take().expect("front is present");
                    let prefix = Prefix::decode(&buf[..range::PREFIX_BYTES]).map_err(invalid)?;
                    let end = range::PREFIX_BYTES + prefix.json_len as usize;
                    if end > buf.len() || buf[end..].iter().any(|&b| b != 0) {
                        return Err(invalid("invalid film summary reserve"));
                    }
                    let front: Front = serde_json::from_slice(&buf[range::PREFIX_BYTES..end])
                        .map_err(|_| invalid("invalid film front summary"))?;
                    if front.ticks_per_second == Some(0) {
                        return Err(invalid("film tick rate must be positive"));
                    }
                    self.events.tick_rate = front.ticks_per_second;
                    self.front_size = Some(prefix.json_len);
                    self.final_head = Some(front.final_head);
                    self.summary = Some(front.summary);
                }
                continue;
            }
            if self.index.is_some() && self.remaining == 0 {
                let limit = if self.front_size.is_some() {
                    range::FOOTER_BYTES
                } else {
                    16
                };
                if bytes.len() > limit - self.footer.len() {
                    return Err(invalid("data after the film index"));
                }
                self.footer.extend_from_slice(bytes);
                self.pos += bytes.len() as u64;
                return Ok(input);
            }
            if self.remaining == 0 {
                let n = bytes.len().min(8 - self.prefix.len());
                self.prefix.extend_from_slice(&bytes[..n]);
                self.pos += n as u64;
                bytes = &bytes[n..];
                if self.prefix.len() != 8 {
                    continue;
                }
                if self.pos == 8 && self.prefix.as_slice() == b"H2BLAM3!" {
                    self.front_buf = Some(std::mem::take(&mut self.prefix));
                    continue;
                }
                self.tag.copy_from_slice(&self.prefix[..4]);
                self.packed = u32::from_le_bytes(self.prefix[4..].try_into().unwrap());
                self.remaining = self.packed as usize;
                self.prefix.clear();
                self.start_chunk()?;
            }
            let n = bytes.len().min(self.remaining);
            if self.collect() {
                self.collected.extend_from_slice(&bytes[..n]);
            }
            self.pos += n as u64;
            self.remaining -= n;
            bytes = &bytes[n..];
            if self.remaining == 0 && self.collect() {
                self.finish_chunk()?;
                self.collected.clear();
                if self.tag == *b"TICK" {
                    return Ok(input - bytes.len());
                }
            }
        }
        Ok(input)
    }

    fn start_chunk(&mut self) -> ApiResult<()> {
        let offset = self.pos - 8;
        if self.remaining == 0 || self.remaining > MAX_CHUNK {
            return Err(invalid("invalid compressed film chunk size"));
        }
        if self.version.is_none() && self.tag != *b"HEAD" {
            return Err(invalid("missing film header"));
        }
        if self.tag == *b"HEAD" {
            self.last_head = Some(offset);
        }
        if self.tag == *b"SUMM" && self.summary.is_some() {
            return Err(invalid("duplicate film summary"));
        }
        if self.cidx.is_some() && self.tag != *b"INDX" {
            return Err(invalid("data between film indexes"));
        }
        if self.tag == *b"INDX" {
            if self.remaining > MAX_INDEX_PACKED {
                return Err(invalid("film index too large"));
            }
            self.index = Some(offset);
        } else {
            self.chunks += 1;
            if self.chunks > MAX_ENTRIES {
                return Err(invalid("too many film chunks"));
            }
            Entry {
                tag: self.tag,
                offset,
                size: self.packed,
                machine_span: None,
            }
            .update(&mut self.observed);
            if self.tag == *b"KEYF" {
                self.keys.update(offset.to_le_bytes());
            }
        }
        let max = if self.version.is_none() || self.tag == *b"SUMM" {
            MAX_SUMMARY_BYTES + 1024
        } else if self.version == Some(3) && self.tag == *b"TICK" {
            MAX_TICK_PACKED
        } else if self.tag == *b"CIDX" {
            MAX_INDEX_PACKED
        } else {
            MAX_CHUNK
        };
        if self.remaining > max {
            return Err(invalid("film chunk too large"));
        }
        Ok(())
    }

    fn collect(&self) -> bool {
        self.version.is_none()
            || matches!(&self.tag, b"SUMM" | b"INDX")
            || (self.version == Some(3) && matches!(&self.tag, b"CIDX" | b"TICK"))
    }

    fn finish_chunk(&mut self) -> ApiResult<()> {
        let offset = self.pos - 8 - u64::from(self.packed);
        if self.version.is_none() {
            let raw = inflate_limited(&self.collected, MAX_SUMMARY_BYTES)?;
            let head: serde_json::Value =
                serde_json::from_slice(&raw).map_err(|_| invalid("invalid film header"))?;
            self.version = head["version"].as_u64().and_then(|n| n.try_into().ok());
            if !matches!(self.version, Some(1..=3))
                || (self.version == Some(3)) != self.front_size.is_some()
            {
                return Err(invalid("unsupported film version"));
            }
            self.events.started_ms = head.pointer("/meta/started_ms").and_then(|v| v.as_u64());
            return Ok(());
        }
        match &self.tag {
            b"INDX" => self.verify_index(),
            b"SUMM" => {
                let bytes = inflate_limited(&self.collected, MAX_SUMMARY_BYTES)?;
                self.summary = Some(
                    serde_json::from_slice(&bytes).map_err(|_| invalid("invalid film summary"))?,
                );
                Ok(())
            }
            b"TICK" => {
                let raw = inflate_limited(&self.collected, MAX_TICK_BYTES)?;
                let (first, last) = self.events.chunk(&raw)?;
                // We already validated each record while importing events.
                // An explicitly stored scene must start at the first tick.
                let chunk = live_proto::film::stream::Chunk::parse(&raw).map_err(invalid)?;
                let pose = chunk
                    .records()
                    .filter_map(Result::ok)
                    .find(|r| r.tag == live_proto::film::stream::POSE);
                self.scene_starts
                    .push(pose.map(|r| r.tick == first && r.payload.first() == Some(&0)));
                digest_tick(
                    &mut self.ticks,
                    first,
                    last,
                    offset,
                    self.packed,
                    raw.len() as u32,
                    &range::clock_runs(&raw).map_err(invalid)?,
                );
                Ok(())
            }
            b"CIDX" => {
                let raw = inflate_limited(&self.collected, MAX_INDEX_BYTES)?;
                let directory: Directory =
                    serde_json::from_slice(&raw).map_err(|_| invalid("invalid cinema index"))?;
                let mut ticks = Sha256::new();
                let mut keys = Sha256::new();
                let mut saw_pose = false;
                for (row, state) in directory.chunks.iter().zip(&self.scene_starts) {
                    // Before the first pose an empty scene is a valid
                    // checkpoint. Once an object scene exists, resetting the
                    // viewer requires an explicit baseline, even if now empty.
                    if (row.scene == row.first && saw_pose && *state != Some(true))
                        || (row.scene != row.first && *state == Some(true))
                    {
                        return Err(invalid("film scene checkpoint has no matching baseline"));
                    }
                    saw_pose |= state.is_some();
                }
                for row in &directory.chunks {
                    digest_tick(
                        &mut ticks,
                        row.first,
                        row.last,
                        row.offset,
                        row.size,
                        row.raw_size,
                        &row.clock,
                    );
                }
                for k in &directory.keyframes {
                    keys.update(k.offset.to_le_bytes());
                }
                if self.ticks.clone().finalize() != ticks.finalize()
                    || self.keys.clone().finalize() != keys.finalize()
                {
                    return Err(invalid(
                        "cinema index does not match stored chunk boundaries",
                    ));
                }
                let mut digest = Sha256::new();
                digest.update(b"CIDX");
                digest.update(self.packed.to_le_bytes());
                digest.update(&self.collected);
                self.cidx = Some(CheckedDirectory {
                    directory,
                    offset,
                    packed_size: self.packed + 8,
                    raw_size: raw.len() as u32,
                    sha256: format!("{:x}", digest.finalize()),
                });
                Ok(())
            }
            _ => unreachable!("only collected tags are decoded"),
        }
    }

    fn verify_index(&self) -> ApiResult<()> {
        let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
            self.collected.as_slice(),
            8 << 20,
        )
        .map_err(|_| invalid("invalid compressed film index"))?;
        let mut reader = decoder.take(MAX_CHUNK as u64 + 1);
        let mut de = serde_json::Deserializer::from_reader(&mut reader);
        let digest = (&mut de)
            .deserialize_seq(IndexDigest)
            .map_err(|_| invalid("invalid film index"))?;
        de.end()
            .map_err(|_| invalid("data after film index entries"))?;
        if reader.limit() == 0
            || !complete_frame(reader.into_inner())
            || self.observed.clone().finalize() != digest.finalize()
        {
            return Err(invalid("film index does not match stored chunks"));
        }
        Ok(())
    }

    pub(super) fn finish(self) -> ApiResult<InspectedFilm> {
        let index = self.index.ok_or_else(|| invalid("missing film index"))?;
        if self.remaining != 0 || !self.prefix.is_empty() || self.front_buf.is_some() {
            return Err(invalid("truncated film"));
        }
        let web_ranges = match self.version {
            Some(1) if self.footer.is_empty() => None,
            Some(2)
                if self.footer.len() == 16
                    && &self.footer[..8] == b"H2FILM2!"
                    && self.footer[8..] == index.to_le_bytes()
                    && self.summary.is_some() =>
            {
                None
            }
            Some(3) => {
                let cidx = self.cidx.ok_or_else(|| invalid("missing cinema index"))?;
                let footer = Footer::decode(&self.footer, self.pos).map_err(invalid)?;
                if self.final_head != self.last_head
                    || footer.cidx_offset != cidx.offset
                    || footer.indx_offset != index
                    || footer.cidx_raw_size != cidx.raw_size
                    || index - cidx.offset != u64::from(cidx.packed_size)
                {
                    return Err(invalid("invalid cinema index bounds"));
                }
                cidx.directory.validate(&footer).map_err(invalid)?;
                Some(WebRanges {
                    size: self.pos,
                    summary_bytes: self.front_size.expect("v3 preamble"),
                    index_offset: cidx.offset,
                    index_size: cidx.packed_size,
                    index_sha256: cidx.sha256,
                    ticks_per_second: self.events.tick_rate,
                })
            }
            _ => return Err(invalid("invalid film footer")),
        };
        let summary = if self.version == Some(1) {
            None
        } else {
            self.summary
        };
        Ok(InspectedFilm {
            summary,
            web_ranges,
            events: self.events,
        })
    }
}

pub(super) fn inflate_limited(input: &[u8], limit: usize) -> ApiResult<Vec<u8>> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(input, 16 << 20)
        .map_err(|_| invalid("invalid compressed film chunk"))?;
    let mut bytes = Vec::new();
    decoder
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("invalid compressed film chunk"))?;
    if bytes.len() > limit || !complete_frame(decoder) {
        return Err(invalid("film chunk exceeds frame limits"));
    }
    Ok(bytes)
}

fn complete_frame(
    decoder: ruzstd::decoding::StreamingDecoder<&[u8], ruzstd::decoding::FrameDecoder>,
) -> bool {
    let (tail, frame) = decoder.into_parts();
    tail.is_empty()
        && frame
            .get_checksum_from_data()
            .is_none_or(|checksum| Some(checksum) == frame.get_calculated_checksum())
}
