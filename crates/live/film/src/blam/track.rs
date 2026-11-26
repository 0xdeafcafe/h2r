use super::{
    DESC, INPT, Keyframe, MAX_SEATS, MatchHeader, RecordData, SUMM, Summary, TIDX, Tick, stream,
    ticks, unified,
};
use crate::{
    Entry, KEYF, Reader, Writer, file::FilmActions,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

/// Ticks after the last scene checkpoint at which the next chunk that
/// begins a scene must take a full public baseline
/// ([`TrackWriter::needs_pose_baseline`]).
pub(crate) const SCENE_REFRESH_TICKS: u64 = 1800;

/// One entry per independent compressed block. For KEYF, count is zero and
/// tick names the restored state. The digest covers the uncompressed payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) tick: u64,
    pub(crate) count: u16,
    pub(crate) offset: u64,
    pub(crate) sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ram_anchor: Option<u64>,
    /// Hash of the adjacent private IPRV, when one was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) identity_sha256: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Directory {
    pub(crate) blocks: Vec<Block>,
    pub(crate) keyframes: Vec<Block>,
}

fn validate_header(descriptor: &MatchHeader) -> Result<()> {
    let mut seats = [false; MAX_SEATS];
    for player in &descriptor.roster {
        let taken = seats
            .get_mut(usize::from(player.seat))
            .context("invalid roster seat")?;
        ensure!(!*taken, "duplicate roster seat");
        *taken = true;
    }
    Ok(())
}

fn validate_summary(summary: &Summary) -> Result<()> {
    ensure!(
        summary
            .players
            .iter()
            .all(|s| usize::from(s.seat) < MAX_SEATS),
        "invalid film score seat"
    );
    Ok(())
}

/// Cloned when a running tape is cut into a game's film: the clone closes
/// the cut and the original keeps recording.
#[derive(Clone, Default)]
pub struct TrackWriter {
    pending: Vec<Tick>,
    extra: Vec<unified::TickRecords>,
    estimated_bytes: usize,
    pose_checkpoint: Option<(u64, usize)>,
    guest_tick: Option<u64>,
    unified: bool,
    split_after_keyframe: bool,
    inputs_in_film: bool,
    directory: Directory,
    tick: u64,
    finished: bool,
}

impl TrackWriter {
    /// A writer whose time chunks carry no actions: they go in the film
    /// file ([`crate::file::FilmWriter`]), and the container's header says
    /// so ([`crate::Header::inputs_in_film`]).
    pub fn without_inputs(mut self) -> Self {
        self.inputs_in_film = true;
        self
    }

    /// A fresh input dictionary starts on the next tick.
    pub fn begins_chunk(&self) -> bool {
        self.pending.is_empty()
            || self.pending.len() >= ticks::MAX_TICKS
            || self.estimated_bytes >= stream::MAX_BYTES / 2
            || self.split_after_keyframe
    }

    /// Request a full scene only at a bounded seek checkpoint. A sleeping
    /// object costs no pose records in the intervening chunks. CIDX points
    /// readers to the first tick of this checkpoint's chunk.
    pub fn needs_pose_baseline(&self) -> bool {
        if !self.begins_chunk() {
            return false;
        }
        let next_chunk = self.directory.blocks.len() + usize::from(!self.pending.is_empty());
        self.pose_checkpoint.is_none_or(|(first, index)| {
            self.tick.saturating_add(1) - first >= SCENE_REFRESH_TICKS
                || next_chunk - index >= crate::range::MAX_SCENE_CHUNKS
        })
    }

    /// Ticks are one-based since boot, including ticks with no active seats.
    pub fn push<W: Write + Seek>(&mut self, out: &mut Writer<W>, tick: Tick) -> Result<()> {
        self.push_with_records(out, tick, Vec::new())
    }

    pub fn push_with_records<W: Write + Seek>(
        &mut self,
        out: &mut Writer<W>,
        tick: Tick,
        records: Vec<RecordData>,
    ) -> Result<()> {
        self.push_clocked(out, tick, records, None)
    }

    /// Record a completed simulation tick and its effective match clock.
    /// The latter matches legacy telemetry clips through countdowns, pauses
    /// and checkpoint reverts. Menu ticks and inactive clocks use None.
    pub fn push_clocked<W: Write + Seek>(
        &mut self,
        out: &mut Writer<W>,
        mut tick: Tick,
        mut records: Vec<RecordData>,
        guest_tick: Option<u64>,
    ) -> Result<()> {
        ensure!(!self.finished, "film track already finished");
        ensure!(
            self.tick.checked_add(1) == Some(tick.tick),
            "nonconsecutive film tick {}",
            tick.tick
        );
        ensure!(
            out.version >= 3
                || (records.is_empty() && guest_tick.is_none() && tick.closing_events.is_empty()),
            "tagged records require film v3"
        );
        let starts_chunk = self.begins_chunk();
        let starts_scene = self.needs_pose_baseline();
        if out.version >= 3 && tick.tick == 1 {
            records.insert(0, RecordData::marker("Start"));
        }
        tick.inputs.sort_unstable_by_key(|seat| seat.seat);
        ticks::slots(&tick)?;
        // Reserve a conservative upper bound for the framing, hash and seat
        // masks. Lifecycle batches can contain user-supplied names and IDs.
        let bytes = 80
            + tick.inputs.len() * 128
            + lifecycle_budget(&tick.events)?
            + lifecycle_budget(&tick.closing_events)?;
        let base = if starts_chunk {
            0
        } else {
            self.estimated_bytes
        };
        let mut size = record_budget(base + bytes, &records)?;
        let continuous = match (self.guest_tick, guest_tick) {
            (None, None) => true,
            (Some(previous), Some(current)) => previous.checked_add(1) == Some(current),
            _ => false,
        };
        if out.version >= 3 && (starts_chunk || !continuous) {
            let mut payload = Vec::new();
            if let Some(guest_tick) = guest_tick {
                stream::write_varint(guest_tick, &mut payload);
            }
            size += payload.len() + 3;
            ensure!(
                size <= stream::MAX_BYTES,
                "film time chunk exceeds record budget"
            );
            records.insert(
                0,
                RecordData {
                    tag: stream::CLOCK,
                    payload,
                },
            );
        }
        if starts_chunk {
            self.flush(out)?;
        }
        if starts_scene {
            self.pose_checkpoint = Some((tick.tick, self.directory.blocks.len()));
        }
        self.unified = out.version >= 3;
        self.guest_tick = guest_tick;
        self.tick = tick.tick;
        self.pending.push(tick);
        self.extra.push(unified::TickRecords {
            during: records,
            after: Vec::new(),
        });
        self.estimated_bytes = size;
        Ok(())
    }

    /// Keep events observed after the last completed simulation update distinct
    /// from observations whose effects the per-tick gameplay hash covers.
    pub fn closing_events(&mut self, events: Vec<super::Event>) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        ensure!(!self.finished, "film track already finished");
        ensure!(self.unified, "closing events require film v3");
        let last = self
            .pending
            .last_mut()
            .context("closing event before first tick")?;
        ticks::validate_events(&events)?;
        let previous = lifecycle_budget(&last.closing_events)?;
        let mut combined = last.closing_events.clone();
        combined.extend(events);
        let size = self.estimated_bytes - previous + lifecycle_budget(&combined)?;
        ensure!(
            size <= stream::MAX_BYTES,
            "film time chunk exceeds record budget"
        );
        last.closing_events = combined;
        self.estimated_bytes = size;
        Ok(())
    }

    /// A final scoreboard may arrive between the last completed tick and
    /// teardown. Keep that tick open until the next tick or finalisation.
    pub fn append_to_last(&mut self, records: Vec<RecordData>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        ensure!(!self.finished, "film track already finished");
        ensure!(self.unified, "tagged records require film v3");
        let last = self
            .extra
            .last_mut()
            .context("record without a completed film tick")?;
        self.estimated_bytes = record_budget(self.estimated_bytes, &records)?;
        last.after.extend(records);
        Ok(())
    }

    fn flush<W: Write + Seek>(&mut self, out: &mut Writer<W>) -> Result<()> {
        if let Some(first) = self.pending.first() {
            let block = if out.version >= 3 {
                let raw = unified::encode(&self.pending, &self.extra, !self.inputs_in_film)?;
                let projection =
                    live_proto::film::privacy::project(&raw).map_err(anyhow::Error::msg)?;
                let identity_sha256 = if let Some(private) = &projection.private {
                    out.chunk_bytes(live_proto::film::privacy::PRIVATE_TAG, private)?;
                    Some(crate::xbe_digest(private))
                } else {
                    None
                };
                let raw = projection.public;
                let has_scene = self
                    .pose_checkpoint
                    .is_some_and(|(tick, _)| tick == first.tick);
                out.chronological_scene(first.tick, self.tick, has_scene, &raw)?;
                Block {
                    tick: first.tick,
                    count: self.pending.len() as u16,
                    offset: out
                        .index
                        .last()
                        .context("missing written time chunk")?
                        .offset,
                    sha256: crate::xbe_digest(&raw),
                    ram_anchor: None,
                    identity_sha256,
                }
            } else {
                let raw = ticks::encode(&self.pending)?;
                write_block(out, INPT, first.tick, self.pending.len() as u16, &raw)?
            };
            self.directory.blocks.push(block);
            self.pending.clear();
            self.extra.clear();
            self.estimated_bytes = 0;
        }
        self.split_after_keyframe = false;
        Ok(())
    }

    /// Call at a quiescent slice boundary after flushing the HLE tape.
    pub fn keyframe<W: Write + Seek>(
        &mut self,
        out: &mut Writer<W>,
        frame: &Keyframe,
    ) -> Result<()> {
        self.keyframe_against(out, frame, frame.ram_anchor.as_ref().map(|(tick, _)| *tick))
    }

    /// Write a snapshot whose RAM section was encoded against the indicated
    /// standalone KEYF. No anchor data is repeated in the target frame.
    pub fn keyframe_against<W: Write + Seek>(
        &mut self,
        out: &mut Writer<W>,
        frame: &Keyframe,
        ram_anchor: Option<u64>,
    ) -> Result<()> {
        if let Some(tick) = ram_anchor {
            let index = self
                .directory
                .keyframes
                .binary_search_by_key(&tick, |k| k.tick)
                .map_err(|_| anyhow::anyhow!("missing film RAM anchor {tick}"))?;
            ensure!(
                out.version >= 3 && self.directory.keyframes[index].ram_anchor.is_none(),
                "film RAM anchor must be standalone"
            );
        }
        ensure!(
            !self.finished && frame.tick == self.tick,
            "keyframe outside current film tick"
        );
        ensure!(
            self.directory
                .keyframes
                .last()
                .is_none_or(|k| k.tick < frame.tick),
            "duplicate or out-of-order film keyframe"
        );
        let raw = encode_keyframe(frame)?;
        if out.version < 3 {
            self.flush(out)?;
        }
        let mut block = write_block(out, KEYF, frame.tick, 0, &raw)?;
        block.ram_anchor = ram_anchor;
        self.directory.keyframes.push(block);
        // A v3 KEYF is written at the exact HLE batch boundary. Its TICK
        // block stays open for end-of-match events at this same tick.
        self.split_after_keyframe = true;
        Ok(())
    }

    /// Flush resolved match settings and scores before the end mark. The
    /// caller writes the final HEAD, MARK and outer index for the whole tape.
    pub fn finish<W: Write + Seek>(
        &mut self,
        out: &mut Writer<W>,
        descriptor: &MatchHeader,
        summary: &Summary,
    ) -> Result<()> {
        ensure!(!self.finished, "film track already finished");
        validate_header(descriptor)?;
        validate_summary(summary)?;
        let bytes = serde_json::to_vec(summary)?;
        ensure!(
            bytes.len() <= live_proto::film::MAX_SUMMARY_BYTES,
            "film summary too large"
        );
        self.flush(out)?;
        // Once the first mandatory section is written, retrying finish would
        // publish duplicate sections even if a later serialisation failed.
        self.finished = true;
        out.chunk(DESC, descriptor)?;
        if out.version >= 3 && summary.players.iter().any(|player| player.xuid.is_some()) {
            out.chunk_bytes(*b"PSUM", &bytes)?;
        }
        out.chunk_bytes(SUMM, &bytes)?;
        out.chunk(TIDX, &self.directory)?;
        Ok(())
    }
}

/// Charge one lifecycle batch, including its framing and subtype. Closing
/// observations can arrive in several calls; charge the final JSON array once.
fn lifecycle_budget(events: &[super::Event]) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }
    ticks::validate_events(events)?;
    let size = serde_json::to_vec(events)?.len();
    ensure!(
        size <= ticks::MAX_EVENT_BYTES,
        "film lifecycle events too large"
    );
    Ok(size + 16)
}

fn record_budget(mut bytes: usize, records: &[RecordData]) -> Result<usize> {
    for record in records {
        ensure!(
            !matches!(record.tag, stream::INPUT | stream::HASH | stream::CLOCK),
            "reserved film record supplied by external producer"
        );
        bytes = bytes
            .checked_add(record.payload.len())
            .and_then(|n| n.checked_add(12))
            .context("film record length overflow")?;
    }
    ensure!(
        bytes <= stream::MAX_BYTES,
        "film time chunk exceeds record budget"
    );
    Ok(bytes)
}

fn write_block<W: Write + Seek>(
    out: &mut Writer<W>,
    tag: crate::Tag,
    tick: u64,
    count: u16,
    raw: &[u8],
) -> Result<Block> {
    out.chunk_bytes(tag, raw)?;
    let offset = out
        .index
        .last()
        .context("missing written film block")?
        .offset;
    Ok(Block {
        tick,
        count,
        offset,
        sha256: crate::xbe_digest(raw),
        ram_anchor: None,
        identity_sha256: None,
    })
}

fn one<R: Read + Seek>(reader: &Reader<R>, tag: crate::Tag) -> Result<Entry> {
    Ok(reader.machine_section().unique(tag)?.clone())
}

/// A stored machine keyframe as the film's private directory lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddedKeyframeInfo {
    pub tick: u64,
    pub offset: u64,
    /// The standalone anchor tick this frame's RAM delta decodes against.
    /// None on a standalone frame, which carries the whole RAM.
    pub ram_anchor: Option<u64>,
}

/// The film's stored machine keyframes in tick order, from the TIDX
/// directory alone. This does not inflate input, HLE or snapshot data.
pub fn embedded_keyframes<R: Read + Seek>(reader: &mut Reader<R>) -> Result<Option<Vec<EmbeddedKeyframeInfo>>> {
    if !reader.machine_section().entries().any(|entry| entry.tag == TIDX) {
        return Ok(None);
    }
    let directory: Directory = reader.read(&one(reader, TIDX)?)?;
    Ok(Some(directory
        .keyframes
        .iter()
        .map(|key| EmbeddedKeyframeInfo {
            tick: key.tick,
            offset: key.offset,
            ram_anchor: key.ram_anchor,
        })
        .collect()))
}

/// Read just the menu block. This does not inflate input, HLE or snapshot data.
pub fn summary<R: Read + Seek>(reader: &mut Reader<R>) -> Result<Summary> {
    if reader.header.version == 3 {
        let menu = reader
            .front
            .as_ref()
            .context("missing v3 film front")?
            .summary
            .clone();
        validate_summary(&menu)?;
        if reader.entries.iter().any(|e| e.tag == *b"PSUM") {
            let entry = one(reader, *b"PSUM")?;
            let bytes = reader.read_bytes_limited(&entry, live_proto::film::MAX_SUMMARY_BYTES)?;
            let summary: Summary = serde_json::from_slice(&bytes)?;
            let mut projected = summary.clone();
            live_proto::film::privacy::public_summary(&mut projected);
            ensure!(
                menu == projected,
                "public and private film summaries differ"
            );
            return Ok(summary);
        }
        return Ok(menu);
    }
    ensure!(
        reader.header.version == 2 && reader.finalized,
        "film has no finished v2 match summary"
    );
    let entry = one(reader, SUMM)?;
    let raw = reader.read_bytes_limited(&entry, live_proto::film::MAX_SUMMARY_BYTES)?;
    let summary = serde_json::from_slice(&raw)?;
    validate_summary(&summary)?;
    Ok(summary)
}

pub struct TrackReader<R: Read + Seek> {
    reader: Reader<R>,
    pub descriptor: MatchHeader,
    pub summary: Summary,
    directory: Directory,
    next_block: usize,
    pending: VecDeque<Tick>,
    film: Option<FilmActions>,
    inputs_in_film: bool,
}

impl TrackReader<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(Reader::open(path)?)
    }

    /// Open a film for playback: its stats container, and its film file
    /// beside it when the container's actions are there.
    pub fn open_with_film(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut track = Self::open(path)?;
        if track.inputs_in_film {
            track.attach_film(FilmActions::open(&crate::file::beside(path))?)?;
        }
        Ok(track)
    }
}

impl<R: Read + Seek> TrackReader<R> {
    pub fn new(mut reader: Reader<R>) -> Result<Self> {
        let summary = summary(&mut reader)?;
        let descriptor = reader.read(&one(&reader, DESC)?)?;
        validate_header(&descriptor)?;
        let directory: Directory = reader.read(&one(&reader, TIDX)?)?;
        // Match rows to every physical time chunk and keyframe in order. A forged tick
        // directory cannot point at an HLE chunk or skip part of the log.
        let unified = reader.header.version >= 3;
        let time_tag = if unified { stream::TAG } else { INPT };
        let mut blocks = directory.blocks.iter();
        let mut keys = directory.keyframes.iter();
        let mut tick = 0u64;
        let mut previous_key = None;
        let mut identity_pending = false;
        for e in &reader.entries {
            ensure!(
                !identity_pending || e.tag == time_tag,
                "orphaned private film events"
            );
            match e.tag {
                tag if tag == time_tag => {
                    let block = blocks.next().context("unindexed film input block")?;
                    ensure!(
                        identity_pending == block.identity_sha256.is_some(),
                        "film identity index mismatch"
                    );
                    identity_pending = false;
                    ensure!(
                        block.offset == e.offset
                            && block.ram_anchor.is_none()
                            && block.count > 0
                            && usize::from(block.count) <= ticks::MAX_TICKS
                            && tick.checked_add(1) == Some(block.tick),
                        "invalid film input block index"
                    );
                    tick = tick
                        .checked_add(u64::from(block.count))
                        .context("film tick overflow")?;
                }
                live_proto::film::privacy::PRIVATE_TAG if unified => identity_pending = true,
                KEYF => {
                    let key = keys.next().context("unindexed film keyframe")?;
                    ensure!(
                        key.count == 0
                            && (unified || key.tick == tick)
                            && key.offset == e.offset
                            && previous_key.is_none_or(|k| k < key.tick),
                        "invalid film keyframe index"
                    );
                    if let Some(base_tick) = key.ram_anchor {
                        let i = directory
                            .keyframes
                            .binary_search_by_key(&base_tick, |k| k.tick)
                            .map_err(|_| anyhow::anyhow!("missing film RAM anchor {base_tick}"))?;
                        ensure!(
                            unified
                                && base_tick < key.tick
                                && directory.keyframes[i].ram_anchor.is_none(),
                            "invalid chained or forward film RAM anchor"
                        );
                    }
                    previous_key = Some(key.tick);
                }
                _ => {}
            }
        }
        ensure!(
            !identity_pending
                && blocks.next().is_none()
                && keys.next().is_none()
                && tick == reader.header.meta.length_ticks,
            "film track length or block count disagrees with header"
        );
        // A v3 keyframe may physically precede the TICK it terminates.
        // Every successor tick must nevertheless start a fresh delta baseline.
        if unified {
            let ranges = reader.ranges.as_ref().context("missing film range index")?;
            for (block, range) in directory.blocks.iter().zip(&ranges.chunks) {
                ensure!(
                    block.tick == range.first
                        && u64::from(block.count) == range.last - range.first + 1,
                    "film tick directories disagree"
                );
            }
            for (key, range) in directory.keyframes.iter().zip(&ranges.keyframes) {
                ensure!(key.tick == range.tick, "film keyframe directories disagree");
            }
            for key in &directory.keyframes {
                ensure!(
                    key.tick <= tick
                        && (key.tick == tick
                            || directory
                                .blocks
                                .binary_search_by_key(&(key.tick + 1), |b| b.tick)
                                .is_ok()),
                    "film keyframe is not at a time chunk boundary"
                );
            }
        }
        let inputs_in_film = reader.header.inputs_in_film;
        Ok(Self {
            reader,
            descriptor,
            summary,
            directory,
            next_block: 0,
            pending: VecDeque::new(),
            film: None,
            inputs_in_film,
        })
    }

    /// Take every tick's actions from the film file instead of the stats
    /// stream's own: events, digests, forge records and pads still come
    /// from the stats stream, and the two must agree on each tick's digest.
    /// Call before the first step or seek.
    pub fn attach_film(&mut self, film: FilmActions) -> Result<()> {
        ensure!(
            film.seed().header.meta.id == self.reader.header.meta.id,
            "film file belongs to another film"
        );
        ensure!(
            film.length_ticks() >= self.reader.header.meta.length_ticks,
            "film file is shorter than the stats stream"
        );
        self.pending.clear();
        self.film = Some(film);
        Ok(())
    }

    /// Last completed tick in the machine timeline, including lobby ticks.
    pub fn length_ticks(&self) -> u64 {
        self.reader.header.meta.length_ticks
    }

    fn decode_block(&mut self, index: usize) -> Result<VecDeque<Tick>> {
        ensure!(
            self.film.is_some() || !self.inputs_in_film,
            "this film's actions are in its film file: open it with TrackReader::open_with_film"
        );
        let mut ticks = self.decode_stats_block(index)?;
        if let Some(film) = &mut self.film {
            for frame in &mut ticks {
                film.fill(frame.tick, &mut frame.inputs, frame.post_hash)?;
            }
        }
        Ok(ticks)
    }

    fn decode_stats_block(&mut self, index: usize) -> Result<VecDeque<Tick>> {
        let block = &self.directory.blocks[index];
        let raw = read_block(&mut self.reader, block)?;
        if self.reader.header.version >= 3 {
            let count = usize::from(block.count);
            let mut public = unified::decode(&raw, block.tick, count)?;
            if let Some(digest) = &block.identity_sha256 {
                let i = self
                    .reader
                    .entries
                    .binary_search_by_key(&block.offset, |e| e.offset)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .context("missing private film events")?;
                let entry = self.reader.entries[i].clone();
                ensure!(
                    entry.tag == live_proto::film::privacy::PRIVATE_TAG,
                    "missing private film events"
                );
                let private = self.reader.read_bytes_limited(&entry, stream::MAX_BYTES)?;
                ensure!(
                    crate::xbe_digest(&private) == *digest,
                    "corrupt private events at tick {}",
                    block.tick
                );
                for (frame, original) in public
                    .iter_mut()
                    .zip(unified::decode(&private, block.tick, count)?)
                {
                    frame.events = original.events;
                    frame.closing_events = original.closing_events;
                }
            }
            Ok(public)
        } else {
            ticks::decode(&raw, block.tick, usize::from(block.count))
        }
    }

    /// Return one complete set of inputs, events and its expected post-tick hash.
    pub fn step(&mut self) -> Result<Option<Tick>> {
        if self.pending.is_empty() {
            if self.next_block == self.directory.blocks.len() {
                return Ok(None);
            }
            self.pending = self.decode_block(self.next_block)?;
            self.next_block += 1;
        }
        Ok(self.pending.pop_front())
    }
}

fn read_block<R: Read + Seek>(reader: &mut Reader<R>, block: &Block) -> Result<Vec<u8>> {
    let index = reader
        .entries
        .binary_search_by_key(&block.offset, |e| e.offset)
        .map_err(|_| anyhow::anyhow!("missing indexed film block"))?;
    let entry = reader.entries[index].clone();
    let raw = if entry.tag == stream::TAG {
        reader.read_bytes_limited(&entry, stream::MAX_BYTES)?
    } else {
        reader.read_bytes(&entry)?
    };
    ensure!(
        crate::xbe_digest(&raw) == block.sha256,
        "corrupt film block at tick {}",
        block.tick
    );
    Ok(raw)
}

pub(crate) fn encode_keyframe(frame: &Keyframe) -> Result<Vec<u8>> {
    ensure!(!frame.snapshot.is_empty(), "empty film snapshot");
    let mut raw = Vec::with_capacity(50);
    for n in [
        frame.tick,
        frame.slices,
        frame.calls,
        frame.lobby_boundary,
        frame.disk_delta.len() as u64,
    ] {
        ticks::varint(n, &mut raw);
    }
    let payload_len = frame
        .disk_delta
        .len()
        .checked_add(frame.snapshot.len())
        .ok_or(crate::ChunkTooLarge)?;
    if payload_len > crate::MAX_CHUNK - raw.len() {
        return Err(crate::ChunkTooLarge.into());
    }
    raw.reserve_exact(payload_len);
    raw.extend_from_slice(&frame.disk_delta);
    raw.extend_from_slice(&frame.snapshot);
    Ok(raw)
}
