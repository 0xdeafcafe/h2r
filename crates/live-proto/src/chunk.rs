//! Recorded frame chunks (docs/live/replay-fidelity.md, "Recording: chunks
//! in the blob store"; docs/live/live-api.md, "Frame chunks").
//!
//! The server records a game's frames as immutable chunks of about
//! [`CHUNK_TICKS`] game ticks (ten seconds): every frame the live stream
//! sent in that span, packed as one stream (a keyframe, then deltas;
//! [`crate::pack`]) and compressed whole. Each chunk is one blob; its
//! [`ChunkMeta`] goes in the log as a server-signed `FrameChunk` event, so
//! the game's chunk list is its index and a seek is one index read plus one
//! chunk fetch.
//!
//! The chunk carries packed records opaquely: it never looks inside a
//! frame beyond the version nibble every packed record starts with, so a
//! new packed frame version needs no change here.
//!
//! ```text
//! magic     "H2CK"
//! format    u8, 1
//! codec     u8: 0 stored, 1 raw deflate (RFC 1951)
//! raw len   LEB128: the container's length before the codec
//! frames    LEB128
//! from tick LEB128: the first frame's
//! to tick   LEB128: the last frame's
//! body      the codec's bytes of a container (crate::pack::push_record)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::game::Frame;
use crate::pack;

/// The `FrameChunk` event type (stream `game-<id>`, written by the server).
pub const FRAME_CHUNK: &str = "FrameChunk";

/// A chunk spans the game ticks of one aligned window this long (ten
/// seconds at 30 ticks a second): a frame whose tick is in the next window
/// starts a new chunk. Ten seconds bounds a seek to one chunk and decoding
/// at most 150 frames.
pub const CHUNK_TICKS: u64 = 300;

/// The deflate level. Packed frames are bit-packed already, so the gain
/// past a fast level is small (`chunk_sizes_per_minute` prints each level);
/// level 6 keeps encoding a ten second chunk around a millisecond on a
/// Worker. Changing it changes no reader.
pub const DEFLATE_LEVEL: u32 = 6;

/// The most bytes a chunk inflates to: well past any ten seconds of
/// frames, and a bound on what a hostile blob can make a reader allocate.
pub const MAX_RAW: usize = 16 << 20;

const MAGIC: &[u8; 4] = b"H2CK";
const FORMAT: u8 = 1;

/// How a chunk's body is coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// The container itself (when deflate would not be smaller).
    Stored,
    /// Raw deflate: the browser's `DecompressionStream("deflate-raw")`.
    Deflate,
}

impl Codec {
    fn byte(self) -> u8 {
        match self {
            Codec::Stored => 0,
            Codec::Deflate => 1,
        }
    }

    fn of(b: u8) -> Option<Self> {
        match b {
            0 => Some(Codec::Stored),
            1 => Some(Codec::Deflate),
            _ => None,
        }
    }
}

/// A chunk's entry in the game's index: the `FrameChunk` event's data.
/// Everything a reader needs to pick chunks without opening them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkMeta {
    /// The first and last frame's game tick.
    pub from_tick: u64,
    pub to_tick: u64,
    pub frames: u32,
    /// The blob's length and SHA-256 (hex): the signature on the event
    /// covers the hash, so the chunk is verifiable.
    pub bytes: u64,
    pub sha256: String,
    pub codec: Codec,
    /// The container's length before the codec.
    pub raw_bytes: u64,
    /// The packed frame versions inside, ascending: a reader that knows
    /// none of them need not fetch the chunk.
    pub versions: Vec<u8>,
    /// Every player index any frame lists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub players: Vec<u32>,
    /// Frames that carry a `cut` (a campaign revert or restart): their
    /// ticks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<u64>,
    /// Hits and players' shots across the frames.
    #[serde(default)]
    pub hits: u32,
    #[serde(default)]
    pub shots: u32,
}

/// A finished chunk: the blob and its index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub meta: ChunkMeta,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    #[error("not a frame chunk")]
    Magic,
    #[error("frame chunk format {0} is not supported")]
    Format(u8),
    #[error("frame chunk codec {0} is not supported")]
    Codec(u8),
    #[error("frame chunk ends early")]
    Short,
    #[error("frame chunk: {0}")]
    Bad(&'static str),
    #[error("{0}")]
    Pack(#[from] pack::PackError),
}

type Result<T> = std::result::Result<T, ChunkError>;

/// The chunk being filled.
struct Open {
    game_id: String,
    encoder: pack::Encoder,
    container: Vec<u8>,
    from_tick: u64,
    to_tick: u64,
    frames: u32,
    versions: BTreeSet<u8>,
    players: BTreeSet<u32>,
    cuts: Vec<u64>,
    hits: u32,
    shots: u32,
}

/// Cuts one game's frames into chunks as they arrive. No timer: a chunk
/// closes when a frame lands in the next window ([`ChunkWriter::push`]) or
/// when the game ends ([`ChunkWriter::finish`]).
pub struct ChunkWriter {
    span: u64,
    open: Option<Open>,
}

impl Default for ChunkWriter {
    fn default() -> Self {
        Self::new(CHUNK_TICKS)
    }
}

impl ChunkWriter {
    /// Chunks of `span` ticks (at least 1).
    pub fn new(span: u64) -> Self {
        ChunkWriter {
            span: span.max(1),
            open: None,
        }
    }

    /// Whether frames are waiting for [`ChunkWriter::finish`].
    pub fn is_empty(&self) -> bool {
        self.open.is_none()
    }

    /// Whether `frame` would be the first of a chunk: nothing is open, or
    /// it closes the open one.
    pub fn starts_chunk(&self, frame: &Frame) -> bool {
        self.open.as_ref().is_none_or(|o| self.closes(o, frame))
    }

    fn closes(&self, o: &Open, frame: &Frame) -> bool {
        frame.tick / self.span != o.from_tick / self.span
            || frame.tick < o.to_tick
            || frame.game_id != o.game_id
    }

    /// Adds a frame. Returns the chunk it closed, if it is the first of a
    /// new window (or of another game, or its tick went back). A frame
    /// that does not pack is skipped, as the live stream skips it.
    pub fn push(&mut self, frame: &Frame) -> Option<Chunk> {
        self.push_with_machine_baseline(frame, None, false)
    }

    /// Supplies the current travel for any keyframe the encoder writes,
    /// including an unexpected keyframe when its string table fills up.
    pub fn push_with_machine_baseline(
        &mut self,
        frame: &Frame,
        baseline: Option<&BTreeMap<u16, crate::game::MachinePosition>>,
        reset: bool,
    ) -> Option<Chunk> {
        let closes = self.open.as_ref().is_some_and(|o| self.closes(o, frame));
        let done = if closes { self.finish() } else { None };
        let open = self.open.get_or_insert_with(|| Open {
            game_id: frame.game_id.clone(),
            // One keyframe at the chunk's head; the encoder sends another
            // only when a delta cannot go (its string table is full).
            encoder: pack::Encoder::new(0, u64::MAX),
            container: Vec::new(),
            from_tick: frame.tick,
            to_tick: frame.tick,
            frames: 0,
            versions: BTreeSet::new(),
            players: BTreeSet::new(),
            cuts: Vec::new(),
            hits: 0,
            shots: 0,
        });
        if reset {
            open.encoder.force_keyframe();
        }
        match open.encoder.encode_with_machine_baseline(frame, baseline) {
            Ok((record, _)) => {
                pack::push_record(&mut open.container, &record);
                open.versions.insert(record[0] & 0x0f);
                if open.frames == 0 {
                    open.from_tick = frame.tick;
                }
                open.to_tick = frame.tick;
                open.frames += 1;
                open.players.extend(frame.players.iter().map(|p| p.index));
                if frame.cut.is_some() {
                    open.cuts.push(frame.tick);
                }
                open.hits = open.hits.saturating_add(frame.hits.len() as u32);
                let shots: u32 = frame.players.iter().filter_map(|p| p.shots).sum();
                open.shots = open.shots.saturating_add(shots);
            }
            Err(_) => {
                if open.frames == 0 {
                    self.open = None;
                }
            }
        }
        done
    }

    /// Closes the open chunk, if it holds a frame.
    pub fn finish(&mut self) -> Option<Chunk> {
        let o = self.open.take()?;
        if o.frames == 0 {
            return None;
        }
        let (codec, body) = match deflate(&o.container, DEFLATE_LEVEL) {
            Some(z) if z.len() < o.container.len() => (Codec::Deflate, z),
            _ => (Codec::Stored, o.container.clone()),
        };
        let mut bytes = Vec::with_capacity(body.len() + 32);
        bytes.extend_from_slice(MAGIC);
        bytes.push(FORMAT);
        bytes.push(codec.byte());
        for v in [
            o.container.len() as u64,
            u64::from(o.frames),
            o.from_tick,
            o.to_tick,
        ] {
            leb128(&mut bytes, v);
        }
        bytes.extend_from_slice(&body);
        let meta = ChunkMeta {
            from_tick: o.from_tick,
            to_tick: o.to_tick,
            frames: o.frames,
            bytes: bytes.len() as u64,
            sha256: crate::sign::hex(&Sha256::digest(&bytes)),
            codec,
            raw_bytes: o.container.len() as u64,
            versions: o.versions.into_iter().collect(),
            players: o.players.into_iter().collect(),
            cuts: o.cuts,
            hits: o.hits,
            shots: o.shots,
        };
        Some(Chunk { meta, bytes })
    }
}

/// Raw deflate at `level`; `None` only if the encoder fails (it does not
/// on a `Vec`).
pub fn deflate(raw: &[u8], level: u32) -> Option<Vec<u8>> {
    let mut z = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(raw.len() / 2),
        flate2::Compression::new(level),
    );
    z.write_all(raw).ok()?;
    z.finish().ok()
}

fn leb128(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_leb128(bytes: &mut &[u8]) -> Result<u64> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = bytes.split_first().ok_or(ChunkError::Short)?;
        *bytes = rest;
        n |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(n);
        }
    }
    Err(ChunkError::Bad("length too long"))
}

/// A chunk's header, read without inflating its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub codec: Codec,
    pub raw_bytes: u64,
    pub frames: u64,
    pub from_tick: u64,
    pub to_tick: u64,
}

/// The header and the coded body.
pub fn header(bytes: &[u8]) -> Result<(Header, &[u8])> {
    let rest = bytes.strip_prefix(MAGIC).ok_or(ChunkError::Magic)?;
    let (&format, rest) = rest.split_first().ok_or(ChunkError::Short)?;
    if format != FORMAT {
        return Err(ChunkError::Format(format));
    }
    let (&codec, mut rest) = rest.split_first().ok_or(ChunkError::Short)?;
    let codec = Codec::of(codec).ok_or(ChunkError::Codec(codec))?;
    let raw_bytes = read_leb128(&mut rest)?;
    let frames = read_leb128(&mut rest)?;
    let from_tick = read_leb128(&mut rest)?;
    let to_tick = read_leb128(&mut rest)?;
    if raw_bytes > MAX_RAW as u64 {
        return Err(ChunkError::Bad("too large"));
    }
    Ok((
        Header {
            codec,
            raw_bytes,
            frames,
            from_tick,
            to_tick,
        },
        rest,
    ))
}

/// The chunk's container of packed records.
pub fn container(bytes: &[u8]) -> Result<Vec<u8>> {
    let (h, body) = header(bytes)?;
    let raw = match h.codec {
        Codec::Stored => body.to_vec(),
        Codec::Deflate => {
            let mut raw = Vec::with_capacity(h.raw_bytes as usize);
            flate2::read::DeflateDecoder::new(body)
                .take(h.raw_bytes + 1)
                .read_to_end(&mut raw)
                .map_err(|_| ChunkError::Bad("deflate"))?;
            raw
        }
    };
    if raw.len() as u64 != h.raw_bytes {
        return Err(ChunkError::Bad("length does not match"));
    }
    Ok(raw)
}

/// Every frame in the chunk, in order.
pub fn frames(bytes: &[u8]) -> Result<Vec<Frame>> {
    let raw = container(bytes)?;
    let mut d = pack::Decoder::new();
    let mut out = Vec::new();
    for record in pack::records(&raw) {
        out.push(d.decode(record?)?.frame);
    }
    Ok(out)
}

/// A chunk of an existing container (records already packed as one
/// stream), for tests and the golden vectors.
#[doc(hidden)]
pub fn seal(container: &[u8], frames: u64, from_tick: u64, to_tick: u64, codec: Codec) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(FORMAT);
    bytes.push(codec.byte());
    for v in [container.len() as u64, frames, from_tick, to_tick] {
        leb128(&mut bytes, v);
    }
    match codec {
        Codec::Stored => bytes.extend_from_slice(container),
        Codec::Deflate => {
            bytes.extend_from_slice(&deflate(container, DEFLATE_LEVEL).expect("deflates"))
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples;

    fn chunks(frames: &[Frame]) -> Vec<Chunk> {
        let mut w = ChunkWriter::default();
        let mut out: Vec<Chunk> = frames.iter().filter_map(|f| w.push(f)).collect();
        out.extend(w.finish());
        out
    }

    fn quantized(frames: &[Frame]) -> Vec<Frame> {
        frames.iter().map(|f| pack::quantize(f).unwrap()).collect()
    }

    #[test]
    fn chunks_hold_every_frame_in_order_and_cut_on_aligned_windows() {
        let frames = samples::multiplayer(450);
        let got = chunks(&frames);
        // Ticks 9000..9898 at 2 a frame: windows 9000, 9300, 9600.
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].meta.from_tick, 9000);
        assert_eq!(got[0].meta.to_tick, 9298);
        assert_eq!(got[0].meta.frames, 150);
        assert_eq!(got[2].meta.to_tick, 9898);
        let mut decoded = Vec::new();
        for c in &got {
            assert_eq!(c.meta.bytes, c.bytes.len() as u64);
            assert_eq!(c.meta.sha256, crate::sign::hex(&Sha256::digest(&c.bytes)));
            assert_eq!(c.meta.versions, vec![pack::VERSION as u8]);
            assert_eq!(c.meta.players, (0..16).collect::<Vec<u32>>());
            let (h, _) = header(&c.bytes).unwrap();
            assert_eq!((h.from_tick, h.to_tick), (c.meta.from_tick, c.meta.to_tick));
            assert_eq!(h.frames, u64::from(c.meta.frames));
            decoded.extend(super::frames(&c.bytes).unwrap());
        }
        assert_eq!(decoded, quantized(&frames));
    }

    #[test]
    fn a_tick_going_back_or_another_game_closes_the_chunk() {
        let mut frames = samples::multiplayer(6);
        frames[3].tick = frames[1].tick;
        frames[5].game_id = "0".repeat(32);
        let got = chunks(&frames);
        assert_eq!(
            got.iter().map(|c| c.meta.frames).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn cuts_hits_and_shots_are_counted() {
        let mut frames = samples::campaign(20);
        frames[7].cut = Some(12);
        let got = chunks(&frames);
        let all: Vec<u64> = got.iter().flat_map(|c| c.meta.cuts.clone()).collect();
        assert_eq!(all, vec![frames[7].tick]);
        let hits: u32 = frames.iter().map(|f| f.hits.len() as u32).sum();
        assert_eq!(got.iter().map(|c| c.meta.hits).sum::<u32>(), hits);
    }

    #[test]
    fn bad_chunks_are_refused() {
        let good = chunks(&samples::multiplayer(4)).remove(0).bytes;
        assert_eq!(frames(b"nope").unwrap_err(), ChunkError::Magic);
        let mut format = good.clone();
        format[4] = 9;
        assert_eq!(frames(&format).unwrap_err(), ChunkError::Format(9));
        let mut codec = good.clone();
        codec[5] = 7;
        assert_eq!(frames(&codec).unwrap_err(), ChunkError::Codec(7));
        assert!(frames(&good[..good.len() - 3]).is_err());
        // A header that claims more than MAX_RAW.
        let mut huge = b"H2CK\x01\x01".to_vec();
        leb128(&mut huge, MAX_RAW as u64 + 1);
        huge.extend_from_slice(&[1, 0, 0]);
        assert_eq!(frames(&huge).unwrap_err(), ChunkError::Bad("too large"));
    }

    #[test]
    fn stored_chunks_read_too() {
        let mut e = pack::Encoder::new(0, u64::MAX);
        let src = samples::multiplayer(5);
        let mut container = Vec::new();
        for f in &src {
            pack::push_record(&mut container, &e.encode(f).unwrap().0);
        }
        let bytes = seal(&container, 5, src[0].tick, src[4].tick, Codec::Stored);
        assert_eq!(frames(&bytes).unwrap(), quantized(&src));
    }

    /// The golden chunk `web/test/fixtures/pack/v7-chunk.bin`: the TypeScript
    /// reader must read it into `chunk.json`'s frames.
    /// `H2_UPDATE_GOLDEN=1` writes them.
    #[test]
    fn golden_chunk_matches_the_writer() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
        let got = chunks(&samples::multiplayer(20)).remove(0);
        assert_eq!(got.meta.codec, Codec::Deflate);
        let json = serde_json::to_string_pretty(&frames(&got.bytes).unwrap()).unwrap() + "\n";
        if std::env::var_os("H2_UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("v7-chunk.bin"), &got.bytes).unwrap();
            std::fs::write(dir.join("v7-chunk.json"), &json).unwrap();
        }
        assert_eq!(
            std::fs::read(dir.join("v7-chunk.bin")).ok().as_deref(),
            Some(&got.bytes[..]),
            "chunk.bin is stale: run with H2_UPDATE_GOLDEN=1"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("v7-chunk.json")).ok(),
            Some(json),
            "chunk.json is stale"
        );
    }

    #[test]
    fn version_six_chunks_still_decode() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
        let bin = std::fs::read(dir.join("v6-chunk.bin")).unwrap();
        let expected: Vec<Frame> =
            serde_json::from_slice(&std::fs::read(dir.join("v6-chunk.json")).unwrap()).unwrap();
        assert_eq!(frames(&bin).unwrap(), expected);
    }

    /// Bytes a minute of play stored as chunks, per mode, against the
    /// live stream and today's once-a-second recording; and the time to
    /// encode and decode one chunk. `cargo test -p live-proto --release
    /// chunk::tests::chunk_sizes_per_minute -- --nocapture` prints the
    /// numbers docs/live/replay-fidelity.md asks for.
    #[test]
    fn chunk_sizes_per_minute() {
        use std::time::Instant;
        // 15 frames a second: 900 a minute; three minutes.
        for (name, frames) in [
            ("16-player multiplayer", samples::multiplayer(2700)),
            ("campaign, 60 actors", samples::campaign(2700)),
        ] {
            let minutes = frames.len() as f64 / 900.0;
            let got = chunks(&frames);
            let stored: u64 = got.iter().map(|c| c.meta.bytes).sum();
            let raw: u64 = got.iter().map(|c| c.meta.raw_bytes).sum();
            // Today: one keyframe a second, each its own record.
            let recorded: usize = frames
                .iter()
                .step_by(15)
                .map(|f| pack::encode_keyframe(f).unwrap().len())
                .sum();
            let mut levels = String::new();
            let containers: Vec<Vec<u8>> =
                got.iter().map(|c| container(&c.bytes).unwrap()).collect();
            for level in [1, 3, 6, 9] {
                let start = Instant::now();
                let z: usize = containers
                    .iter()
                    .map(|c| deflate(c, level).unwrap().len())
                    .sum();
                let us = start.elapsed().as_secs_f64() * 1e6 / containers.len() as f64;
                levels += &format!(
                    " L{level} {:.0} KB/min ({us:.0} us a chunk);",
                    z as f64 / minutes / 1000.0
                );
            }
            let start = Instant::now();
            let mut n = 0;
            for c in &got {
                n += super::frames(&c.bytes).unwrap().len();
            }
            let decode_us = start.elapsed().as_secs_f64() * 1e6 / got.len() as f64;
            println!(
                "{name}: {} chunks; stored {:.0} KB/min (raw {:.0} KB/min, deflate {:.2}x); \
                 recorded keyframes today {:.0} KB/min; decode {decode_us:.0} us a chunk;{levels}",
                got.len(),
                stored as f64 / minutes / 1000.0,
                raw as f64 / minutes / 1000.0,
                stored as f64 / raw as f64,
                recorded as f64 / minutes / 1000.0,
            );
            assert_eq!(n, frames.len());
            // Never bigger than the container, and every frame at 15 Hz
            // costs less than a deflated keyframe a second would.
            assert!(stored <= raw + 32 * got.len() as u64);
        }
    }
}
