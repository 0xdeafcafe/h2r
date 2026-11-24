//! Sealing an unsealed tape a dead session left
//! (docs/game/film-format.md, "An exit without a teardown still seals").
//! A hard kill stops the recorder between chunks, so the file holds the
//! envelopes written so far and none of the indexes a film needs. Recovery
//! rewrites the playable prefix as a film of its own: the envelopes up to
//! an exact end, then the seal a live recorder writes.
//!
//! The end is the last D3D capture mark whose tick the tape actually
//! flushed. The recorder flushes its call batch and every checkpoint
//! before such a mark, so the mark names a moment whose slice, tick and
//! call counts are all on the tape. Up to 127 ticks can sit unflushed in
//! the track writer at death, so a capture mark may name a tick whose
//! chunk never landed; such a mark cannot end a film and recovery falls
//! back to the last one that does. Everything past the chosen end is
//! dropped even when it sits earlier in the file (a checkpoint or a call
//! batch recorded past the end), so the replay is never asked for state
//! the tape does not hold. The ticks past the end inside a kept time
//! chunk stay: a chunk is never split, and the replay stops at the end
//! before reading them.
//!
//! The recovered film keeps no map, variant or scores: the descriptors of
//! an unsealed tape live in the recorder's memory, not in the file. It
//! lists unnamed and plays from boot. A tape with no usable capture mark
//! — a session that died outside a game, or before its first capture —
//! has no exact end; [`recover_tape`] says so and the caller deletes it.

use crate::{
    Entry, HEAD, HASH, KEYF, MARK, Reader, Tag, Writer,
    blam::{self, MatchHeader, Summary, TidxBlock, TidxDirectory},
    range,
    tape::{Check, Mark},
    xbe_digest,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const PRIVATE: Tag = live_proto::film::privacy::PRIVATE_TAG;

/// The chunks a tape holds, as far as they read whole. A tail the dead
/// session was still writing ends the walk; a zeroed or torn region reads
/// as a zero size or a length past the end and ends it too.
fn walk(input: &mut File, end: u64) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut offset = range::DATA_OFFSET;
    while end.saturating_sub(offset) >= 8 {
        input.seek(SeekFrom::Start(offset))?;
        let mut frame = [0u8; 8];
        input.read_exact(&mut frame)?;
        let tag: Tag = frame[..4].try_into()?;
        let size = u32::from_le_bytes(frame[4..].try_into()?);
        if size == 0 || u64::from(size) > end - offset - 8 {
            break;
        }
        entries.push(Entry { tag, offset, size });
        offset += 8 + u64::from(size);
    }
    Ok(entries)
}

/// The first and last tick of one time chunk's public payload.
fn tick_span(input: &mut File, entry: &Entry) -> Result<(u64, u64)> {
    let raw = crate::read_bytes(input, entry, crate::MAX_CHUNK)?;
    let block = blam::stream::Chunk::parse(&raw).map_err(anyhow::Error::msg)?;
    ensure!(block.tick_count > 0, "a time chunk holds no tick");
    Ok((
        block.first_tick,
        block.first_tick + u64::from(block.tick_count - 1),
    ))
}

/// Seal the unsealed v3 tape at `path` in place as a film of its own, and
/// return whether it held a playable game. The film is written to a
/// temporary sibling and renamed over the tape only once it reads back
/// whole, so a failure leaves the tape as it was.
pub fn recover_tape(path: &Path) -> Result<bool> {
    let mut source = File::options().read(true).open(path)?;
    let end = source.seek(SeekFrom::End(0))?;
    let entries = walk(&mut source, end)?;
    let Some((head, rest)) = entries.split_first() else {
        return Ok(false);
    };
    ensure!(head.tag == HEAD, "the tape does not begin with a header");
    let boot: crate::Header = crate::decode(&mut source, head)?;
    ensure!(boot.version == 3, "only a v3 tape can be sealed");

    // The last tick the tape flushed, then the last capture mark it covers.
    let mut spans = HashMap::with_capacity(rest.len());
    for entry in rest.iter().filter(|e| e.tag == range::TICK) {
        spans.insert(entry.offset, tick_span(&mut source, entry)?);
    }
    let flushed = spans.values().map(|(_, last)| *last).max().unwrap_or(0);
    let mut anchor = None;
    for entry in rest.iter().filter(|e| e.tag == MARK) {
        if let Mark::D3dBarrier { slices, tick, .. } = crate::decode::<_, Mark>(&mut source, entry)?
            && tick <= flushed
        {
            anchor = Some((slices, tick, entry.offset));
        }
    }
    let Some((end_slices, end_tick, mark_offset)) = anchor else {
        return Ok(false);
    };

    // Keep what the replay reads up to the end. A tape whose session died
    // while sealing holds closing chunks of its own; recovery writes fresh
    // ones, so those are dropped.
    let chunk_kept = |entry: &Entry| {
        spans
            .get(&entry.offset)
            .is_some_and(|(first, _)| *first <= end_tick)
    };
    // The machine index opens with the first header, so it is kept; the
    // sealed header goes at the end like a live recorder's.
    let mut kept: Vec<Entry> = Vec::with_capacity(entries.len());
    kept.push(head.clone());
    for (i, entry) in rest.iter().enumerate() {
        let keep = match entry.tag {
            t if t == range::TICK => chunk_kept(entry),
            PRIVATE => rest.get(i + 1).is_some_and(|next| {
                next.tag == range::TICK && spans.get(&next.offset).is_some_and(|(f, _)| *f <= end_tick)
            }),
            HASH => crate::decode::<_, Check>(&mut source, entry).is_ok_and(|c| c.tick <= end_tick),
            MARK => match crate::decode::<_, Mark>(&mut source, entry) {
                Ok(Mark::D3dBarrier { tick, .. }) => tick <= end_tick,
                Ok(Mark::Lobby { .. }) => entry.offset < mark_offset,
                _ => false,
            },
            crate::tape::TAPE => entry.offset < mark_offset,
            KEYF => crate::read_bytes(&mut source, entry, crate::MAX_CHUNK)
                .ok()
                .and_then(|raw| range::key_tick(&raw).ok())
                .is_some_and(|tick| tick <= end_tick),
            t if t == blam::DESC || t == blam::SUMM || t == blam::TIDX => false,
            t if t == *b"PSUM" || t == range::CIDX || t == crate::INDX => false,
            crate::HEAD | crate::DATA => true,
            _ => true,
        };
        if keep {
            kept.push(entry.clone());
        }
    }

    // The recorder flushes its call batch before a capture mark, so the
    // first taped call after the mark names the call count at the end.
    let mut calls = 0;
    for entry in rest.iter().filter(|e| e.offset > mark_offset) {
        if entry.tag == crate::tape::TAPE {
            let batch = crate::tape::decode_batch(&crate::read_bytes(&mut source, entry, crate::MAX_CHUNK)?)?;
            calls = batch.first().context("an empty film call batch")?.index;
            break;
        }
    }
    if calls == 0 {
        for entry in kept.iter().filter(|e| e.tag == crate::tape::TAPE) {
            let batch = crate::tape::decode_batch(&crate::read_bytes(&mut source, entry, crate::MAX_CHUNK)?)?;
            if let Some(last) = batch.last() {
                calls = last.index + 1;
            }
        }
    }

    // Rewrite the kept envelopes densely: a film's chunks are contiguous.
    // Two sessions recovering the same library must not share a staging
    // file.
    let staging = path.with_extension(format!("{}.recover", live_proto::sign::random_id()));
    let mut out = File::create(&staging)?;
    out.write_all(&range::Prefix { json_len: 0 }.encode())?;
    out.seek(SeekFrom::Start(range::DATA_OFFSET))?;
    let mut index = Vec::with_capacity(kept.len());
    for entry in &kept {
        let at = out.stream_position()?;
        source.seek(SeekFrom::Start(entry.offset))?;
        let mut frame = [0u8; 8];
        source.read_exact(&mut frame)?;
        ensure!(
            frame[..4] == entry.tag && u32::from_le_bytes(frame[4..].try_into()?) == entry.size,
            "the tape changed while it was recovered"
        );
        let mut bytes = vec![0u8; entry.size as usize];
        source.read_exact(&mut bytes)?;
        out.write_all(&frame)?;
        out.write_all(&bytes)?;
        index.push(Entry { tag: entry.tag, offset: at, size: entry.size });
    }

    // Both directories, from the rewritten chunks: the public one for the
    // menu and web, the private one for playback. The scene checkpoints
    // are placed the way the writer placed them, from the chunks alone.
    let mut chunks = Vec::new();
    let mut blocks = Vec::new();
    let mut keys = Vec::new();
    let mut key_blocks = Vec::new();
    for (entry, at) in kept.iter().zip(&index) {
        let raw = crate::read_bytes(&mut source, entry, crate::MAX_CHUNK)?;
        if entry.tag == range::TICK {
            let block = blam::stream::Chunk::parse(&raw).map_err(anyhow::Error::msg)?;
            ensure!(block.tick_count > 0, "a time chunk holds no tick");
            let first = block.first_tick;
            let last = first + u64::from(block.tick_count - 1);
            chunks.push(range::Chunk {
                clock: range::clock_runs(&raw).map_err(anyhow::Error::msg)?,
                scene: first,
                first,
                last,
                offset: at.offset,
                size: at.size,
                raw_size: raw.len() as u32,
            });
            blocks.push(TidxBlock {
                tick: first,
                count: block.tick_count,
                offset: at.offset,
                sha256: xbe_digest(&raw),
                ram_anchor: None,
                identity_sha256: None,
            });
        } else if entry.tag == KEYF {
            let tick = range::key_tick(&raw)?;
            keys.push(range::Keyframe { tick, offset: at.offset });
            key_blocks.push(TidxBlock {
                tick,
                count: 0,
                offset: at.offset,
                sha256: xbe_digest(&raw),
                ram_anchor: None,
                identity_sha256: None,
            });
        }
    }
    // Each time chunk's identity digest comes from the private payload the
    // recorder wrote just before it.
    let mut identity = None;
    let mut next_block = 0;
    for entry in &kept {
        if entry.tag == PRIVATE {
            let raw = crate::read_bytes(&mut source, entry, crate::MAX_CHUNK)?;
            identity = Some(xbe_digest(&raw));
        } else if entry.tag == range::TICK {
            blocks[next_block].identity_sha256 = identity.take();
            next_block += 1;
        } else {
            identity = None;
        }
    }
    let mut scene_tick = 0;
    let mut scene_index = 0;
    for (i, chunk) in chunks.iter_mut().enumerate() {
        if i == 0
            || chunk.first - scene_tick >= blam::SCENE_REFRESH_TICKS
            || i - scene_index >= range::MAX_SCENE_CHUNKS
        {
            scene_tick = chunk.first;
            scene_index = i;
        }
        chunk.scene = scene_tick;
    }

    let mut header = boot.clone();
    header.meta.length_ticks = end_tick;
    let mut ranges = range::Build::new(&boot);
    ranges.directory = range::Directory { version: 1, chunks, keyframes: keys };
    let mut writer = Writer {
        out,
        index,
        failed: false,
        version: 3,
        ranges: Some(Box::new(ranges)),
        mirror: None,
    };
    writer.chunk(HEAD, &header)?;
    writer.chunk(blam::DESC, &MatchHeader::default())?;
    writer.chunk_bytes(blam::SUMM, &serde_json::to_vec(&Summary::default())?)?;
    writer.chunk(blam::TIDX, &TidxDirectory { blocks, keyframes: key_blocks })?;
    writer.chunk(MARK, &Mark::End { slices: end_slices, tick: end_tick, calls })?;
    let file = writer.finish()?;
    file.sync_all()?;
    drop(file);

    // Rename over the tape only once the result reads back whole.
    let playback = match Reader::open(&staging).and_then(crate::tape::Playback::load) {
        Ok(playback) => playback,
        Err(error) => {
            let _ = fs::remove_file(&staging);
            return Err(error.context("the recovered film does not read back"));
        }
    };
    ensure!(
        playback.end == Some((end_slices, end_tick, calls)),
        "the recovered film ends elsewhere"
    );
    drop(playback);
    // A tape whose actions are in a film file ends there at the same tick.
    if boot.inputs_in_film
        && let Err(error) = crate::file::recover_beside(path, end_tick)
    {
        let _ = fs::remove_file(&staging);
        return Err(error.context("the recovered film's film file"));
    }
    fs::rename(&staging, path)?;
    Ok(true)
}
