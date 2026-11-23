//! The film file (docs/game/input-film.md, "Layout"): a seed, the seats'
//! action columns in chunks of 256 ticks ([`crate::columns`]), the machine
//! part's envelopes, and a trailer that finds a chunk without a search.
//!
//! ```text
//! header, 64 bytes, little-endian
//!    0   8  "H2FILM4!"
//!    8   u16 version (4), u16 flags (bit 0 machine part, bit 1 sealed,
//!           bit 2 begins at a start point)
//!   12   u8 ticks per second, u8 seat limit (16), u16 ticks per chunk (256)
//!   16  16  film id (the first 16 bytes of the SHA-256 of its text id)
//!   32   u32 length in ticks, u32 stored bytes of the seed
//!   40   u64 offset of the trailer
//!   48  16  digest of the dictionary the chunks name, or zero
//! seed      one zstd frame of JSON: the film header and the match descriptor
//! blocks    u8 kind (0 tick chunk, 1 machine envelope, 2 start point),
//!           u32 stored bytes, then the payload: a tick chunk is its zstd
//!           frame; a machine envelope is its four-byte tag and the zstd
//!           frame it had in the container; a start point is a zstd frame
//! trailer   u32 chunk count, then a u64 offset per tick chunk
//! ```
//!
//! The length, the trailer offset and the sealed flag are zero while a file
//! is written; a reader walks the blocks and stops at a torn last one.
//! Chunk i covers ticks 256 i + 1 to 256 i + 256. A tick's digest here is
//! the low 16 bits of the gameplay digest, and a chunk ends with the last
//! tick's whole digest.
use crate::{
    Entry, Header, Reader, Tag,
    blam::{self, Event, MatchHeader, TrackReader},
    columns::{ACTION_BYTES, CHUNK_TICKS, ChunkDecoder, ChunkEncoder, FRAME_CAP, SEATS},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAGIC: &[u8; 8] = b"H2FILM4!";
pub const VERSION: u16 = 4;
pub const HEADER_BYTES: usize = 64;
pub const FLAG_MACHINE: u16 = 1;
pub const FLAG_SEALED: u16 = 2;
pub const FLAG_START_POINT: u16 = 4;
const KIND_TICKS: u8 = 0;
const KIND_MACHINE: u8 = 1;
const KIND_START: u8 = 2;
const MAX_SEED: usize = 1 << 20;
const MAX_START: u32 = 4 << 20;
/// Seat record kinds the recorder writes: a seat joined (its seat number
/// and kind), a seat left or quit (its seat number).
pub const RECORD_JOIN: u8 = 1;
pub const RECORD_LEAVE: u8 = 2;

/// Tick flags: the digest is present; events (in the stats stream) were
/// recorded on the tick.
pub const TICK_DIGEST: u8 = 1;
pub const TICK_EVENTS: u8 = 2;

/// Everything a player needs before the first tick besides the machine
/// part: what the film's header says, and the match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seed {
    pub header: Header,
    pub descriptor: MatchHeader,
}

fn film_id(id: &str) -> [u8; 16] {
    let hex = crate::xbe_digest(id.as_bytes());
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    out
}

struct Head {
    flags: u16,
    ticks_per_second: u8,
    id: [u8; 16],
    length: u32,
    seed_len: u32,
    trailer: u64,
}

impl Head {
    fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut b = [0u8; HEADER_BYTES];
        b[..8].copy_from_slice(MAGIC);
        b[8..10].copy_from_slice(&VERSION.to_le_bytes());
        b[10..12].copy_from_slice(&self.flags.to_le_bytes());
        b[12] = self.ticks_per_second;
        b[13] = SEATS as u8;
        b[14..16].copy_from_slice(&(CHUNK_TICKS as u16).to_le_bytes());
        b[16..32].copy_from_slice(&self.id);
        b[32..36].copy_from_slice(&self.length.to_le_bytes());
        b[36..40].copy_from_slice(&self.seed_len.to_le_bytes());
        b[40..48].copy_from_slice(&self.trailer.to_le_bytes());
        b
    }

    fn decode(b: &[u8; HEADER_BYTES]) -> Result<Self> {
        ensure!(&b[..8] == MAGIC, "not a film file");
        let version = u16::from_le_bytes([b[8], b[9]]);
        ensure!(
            version == VERSION,
            "film file version {version} is not version {VERSION}"
        );
        ensure!(
            usize::from(b[13]) == SEATS
                && usize::from(u16::from_le_bytes([b[14], b[15]])) == CHUNK_TICKS,
            "film file has other seat or chunk limits"
        );
        ensure!(
            b[48..64].iter().all(|&x| x == 0),
            "film file names a dictionary this build lacks"
        );
        let flags = u16::from_le_bytes([b[10], b[11]]);
        ensure!(flags >> 3 == 0, "film file has unknown flags");
        Ok(Self {
            flags,
            ticks_per_second: b[12],
            id: b[16..32].try_into()?,
            length: u32::from_le_bytes(b[32..36].try_into()?),
            seed_len: u32::from_le_bytes(b[36..40].try_into()?),
            trailer: u64::from_le_bytes(b[40..48].try_into()?),
        })
    }
}

pub struct FilmWriter<W: Write + Seek> {
    out: W,
    encoder: ChunkEncoder,
    chunks: Vec<u64>,
    ticks: u64,
    last_digest: u64,
    head: Head,
    /// Where the blocks start, after the seed.
    body: u64,
    seed_id: String,
    /// A cut: its last chunk is written, and no tick follows.
    closed: bool,
}

impl<W: Write + Seek> FilmWriter<W> {
    pub fn new(mut out: W, seed: &Seed, ticks_per_second: u8) -> Result<Self> {
        let json = serde_json::to_vec(seed)?;
        ensure!(json.len() <= MAX_SEED, "film seed is too large");
        let frame = zstd::bulk::compress(&json, 3)?;
        let head = Head {
            flags: 0,
            ticks_per_second,
            id: film_id(&seed.header.meta.id),
            length: 0,
            seed_len: u32::try_from(frame.len())?,
            trailer: 0,
        };
        out.write_all(&head.encode())?;
        out.write_all(&frame)?;
        Ok(Self {
            out,
            encoder: ChunkEncoder::new()?,
            chunks: Vec::new(),
            ticks: 0,
            last_digest: 0,
            head,
            body: (HEADER_BYTES + frame.len()) as u64,
            seed_id: seed.header.meta.id.clone(),
            closed: false,
        })
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// One tick: its flags, its gameplay digest and each seat's action. A
    /// chunk closes on its 256th.
    pub fn push_tick(
        &mut self,
        flags: u8,
        digest: u64,
        seats: &[Option<[u8; ACTION_BYTES]>; SEATS],
    ) -> Result<()> {
        ensure!(!self.closed, "a cut film takes no more ticks");
        // A full chunk stays open until the next tick, so a seat record can
        // still follow the 256th.
        if self.encoder.ticks() == CHUNK_TICKS {
            self.close_chunk()?;
        }
        self.encoder.push_tick(flags, digest, seats)?;
        self.ticks += 1;
        self.last_digest = digest;
        Ok(())
    }

    /// A seat record on the tick just pushed, even the last of a chunk.
    pub fn seat_record(&mut self, seat: u8, kind: u8, bytes: &[u8]) -> Result<()> {
        let n = self.encoder.ticks();
        ensure!(n > 0 && !self.closed, "a seat record needs a tick");
        self.encoder.seat_record(n - 1, seat, kind, bytes)
    }

    /// A machine envelope: its tag and the zstd frame it is stored as.
    pub fn machine(&mut self, tag: Tag, frame: &[u8]) -> Result<()> {
        self.head.flags |= FLAG_MACHINE;
        let mut payload = Vec::with_capacity(4 + frame.len());
        payload.extend_from_slice(&tag);
        payload.extend_from_slice(frame);
        self.block(KIND_MACHINE, &payload)
    }

    /// A start point: the state a film that begins mid-game starts from.
    pub fn start_point(&mut self, frame: &[u8]) -> Result<()> {
        ensure!(self.ticks == 0, "a start point comes before the first tick");
        self.head.flags |= FLAG_START_POINT;
        self.block(KIND_START, frame)
    }

    fn block(&mut self, kind: u8, payload: &[u8]) -> Result<()> {
        self.out.write_all(&[kind])?;
        self.out
            .write_all(&u32::try_from(payload.len())?.to_le_bytes())?;
        self.out.write_all(payload)?;
        Ok(())
    }

    fn close_chunk(&mut self) -> Result<()> {
        let at = self.out.stream_position()?;
        let frame = self.encoder.finish(self.last_digest)?;
        self.out.write_all(&[KIND_TICKS])?;
        self.out
            .write_all(&u32::try_from(frame.len())?.to_le_bytes())?;
        self.out.write_all(frame)?;
        self.chunks.push(at);
        Ok(())
    }

    /// Close the last chunk, write the trailer and seal the header.
    pub fn finish(mut self) -> Result<W> {
        if self.encoder.ticks() > 0 {
            self.close_chunk()?;
        }
        let trailer = self.out.stream_position()?;
        self.out
            .write_all(&(self.chunks.len() as u32).to_le_bytes())?;
        for &offset in &self.chunks {
            self.out.write_all(&offset.to_le_bytes())?;
        }
        self.head.flags |= FLAG_SEALED;
        self.head.length = u32::try_from(self.ticks)?;
        self.head.trailer = trailer;
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&self.head.encode())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Read + Write + Seek> FilmWriter<W> {
    /// Cut a film of its own from this one as it stands, into `out`: the
    /// `seed`'s film (a new ID), every block written so far, and a copy of
    /// the chunk still open. This film keeps recording, untouched. The cut
    /// takes machine envelopes ([`FilmWriter::machine`]) and
    /// [`FilmWriter::finish`], but no more ticks.
    pub fn fork<V: Write + Seek>(&mut self, mut out: V, seed: &Seed) -> Result<FilmWriter<V>> {
        ensure!(
            seed.header.meta.id != self.seed_id,
            "a cut has an ID of its own"
        );
        let json = serde_json::to_vec(seed)?;
        ensure!(json.len() <= MAX_SEED, "film seed is too large");
        let frame = zstd::bulk::compress(&json, 3)?;
        let mut head = Head {
            flags: self.head.flags,
            ticks_per_second: self.head.ticks_per_second,
            id: film_id(&seed.header.meta.id),
            length: 0,
            seed_len: u32::try_from(frame.len())?,
            trailer: 0,
        };
        head.flags &= !FLAG_SEALED;
        out.write_all(&head.encode())?;
        out.write_all(&frame)?;
        let shift = (HEADER_BYTES + frame.len()) as u64;
        let end = self.out.stream_position()?;
        self.out.seek(SeekFrom::Start(self.body))?;
        let copied = std::io::copy(&mut (&mut self.out).take(end - self.body), &mut out)?;
        self.out.seek(SeekFrom::Start(end))?;
        ensure!(copied == end - self.body, "film file is shorter than written");
        let mut chunks: Vec<u64> = self.chunks.iter().map(|&at| at - self.body + shift).collect();
        if self.encoder.ticks() > 0 {
            let at = out.stream_position()?;
            let frame = self.encoder.finish_copy(self.last_digest)?;
            out.write_all(&[KIND_TICKS])?;
            out.write_all(&u32::try_from(frame.len())?.to_le_bytes())?;
            out.write_all(&frame)?;
            chunks.push(at);
        }
        Ok(FilmWriter {
            out,
            encoder: ChunkEncoder::new()?,
            chunks,
            ticks: self.ticks,
            last_digest: self.last_digest,
            head,
            body: shift,
            seed_id: seed.header.meta.id.clone(),
            closed: true,
        })
    }
}

/// The machine envelopes of a container being written, as a [`crate::Mirror`]
/// into `film`: each chunk the film carries ([`carried`]) is stored in the
/// film file as the container stores it.
pub fn machine_mirror<W: Write + Seek + Send + 'static>(
    film: Arc<Mutex<FilmWriter<W>>>,
) -> crate::Mirror {
    Box::new(move |tag, frame| {
        if carried(tag) {
            film.lock()
                .map_err(|_| anyhow::anyhow!("film file writer is poisoned"))?
                .machine(tag, frame)?;
        }
        Ok(())
    })
}

/// A machine envelope in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineBlock {
    pub tag: Tag,
    /// Where the stored frame starts, and its length.
    pub offset: u64,
    pub len: u32,
}

pub struct FilmReader<R: Read + Seek> {
    input: R,
    pub seed: Seed,
    pub flags: u16,
    pub ticks_per_second: u8,
    pub sealed: bool,
    /// The film's length in ticks: the header's once sealed, else a count of
    /// the ticks of the whole chunks found.
    pub length_ticks: u64,
    chunks: Vec<(u64, u32)>,
    pub machine: Vec<MachineBlock>,
    pub start_points: Vec<MachineBlock>,
}

fn read_exact_at<R: Read + Seek>(input: &mut R, at: u64, buf: &mut [u8]) -> Result<()> {
    input.seek(SeekFrom::Start(at))?;
    input.read_exact(buf)?;
    Ok(())
}

impl<R: Read + Seek> FilmReader<R> {
    /// Read the header and seed and find every block, without inflating a
    /// chunk. A sealed file is checked against its trailer; one that is
    /// not sealed ends at its last whole block.
    pub fn open(mut input: R) -> Result<Self> {
        let file_len = input.seek(SeekFrom::End(0))?;
        ensure!(file_len >= HEADER_BYTES as u64, "film file is cut short");
        let mut raw = [0u8; HEADER_BYTES];
        read_exact_at(&mut input, 0, &mut raw)?;
        let head = Head::decode(&raw)?;
        let sealed = head.flags & FLAG_SEALED != 0;
        let seed_end = HEADER_BYTES as u64 + u64::from(head.seed_len);
        ensure!(
            head.seed_len as usize <= MAX_SEED && seed_end <= file_len,
            "film seed is cut short or too large"
        );
        let mut frame = vec![0u8; head.seed_len as usize];
        read_exact_at(&mut input, HEADER_BYTES as u64, &mut frame)?;
        let json =
            zstd::bulk::decompress(&frame, MAX_SEED).context("film seed does not inflate")?;
        let seed: Seed = serde_json::from_slice(&json).context("film seed is not a seed")?;
        ensure!(
            head.id == film_id(&seed.header.meta.id),
            "film seed is not this film's"
        );
        let end = if sealed {
            ensure!(
                head.trailer >= seed_end && head.trailer + 4 <= file_len,
                "film trailer is outside the file"
            );
            head.trailer
        } else {
            file_len
        };
        let mut chunks = Vec::new();
        let mut machine = Vec::new();
        let mut start_points = Vec::new();
        let mut at = seed_end;
        let mut prefix = [0u8; 5];
        while at + 5 <= end {
            read_exact_at(&mut input, at, &mut prefix)?;
            let kind = prefix[0];
            let stored = u32::from_le_bytes([prefix[1], prefix[2], prefix[3], prefix[4]]);
            let body = at + 5;
            if body + u64::from(stored) > end {
                ensure!(!sealed, "film block runs past the trailer");
                break;
            }
            match kind {
                KIND_TICKS => {
                    ensure!(
                        stored as usize <= FRAME_CAP,
                        "film tick chunk is larger than a chunk can be"
                    );
                    chunks.push((body, stored));
                }
                KIND_MACHINE => {
                    ensure!(
                        stored >= 4 && stored as usize <= crate::MAX_CHUNK + 4,
                        "film machine envelope has a bad size"
                    );
                    let mut tag = [0u8; 4];
                    read_exact_at(&mut input, body, &mut tag)?;
                    machine.push(MachineBlock {
                        tag,
                        offset: body + 4,
                        len: stored - 4,
                    });
                }
                KIND_START => {
                    ensure!(stored <= MAX_START, "film start point is too large");
                    start_points.push(MachineBlock {
                        tag: *b"STRT",
                        offset: body,
                        len: stored,
                    });
                }
                _ => bail!("film block of unknown kind {kind}"),
            }
            at = body + u64::from(stored);
        }
        let length_ticks = if sealed {
            ensure!(at == end, "film blocks do not end at the trailer");
            let mut count = [0u8; 4];
            read_exact_at(&mut input, head.trailer, &mut count)?;
            let count = u32::from_le_bytes(count) as usize;
            ensure!(
                count == chunks.len() && head.trailer + 4 + 8 * count as u64 == file_len,
                "film trailer disagrees with its blocks"
            );
            let mut rows = vec![0u8; 8 * count];
            read_exact_at(&mut input, head.trailer + 4, &mut rows)?;
            for (row, &(offset, _)) in rows.chunks_exact(8).zip(&chunks) {
                ensure!(
                    u64::from_le_bytes(row.try_into()?) + 5 == offset,
                    "film trailer disagrees with its blocks"
                );
            }
            let length = u64::from(head.length);
            ensure!(
                length.div_ceil(CHUNK_TICKS as u64) == count as u64,
                "film length disagrees with its chunks"
            );
            length
        } else {
            (chunks.len() * CHUNK_TICKS) as u64
        };
        Ok(Self {
            input,
            seed,
            flags: head.flags,
            ticks_per_second: head.ticks_per_second,
            sealed,
            length_ticks,
            chunks,
            machine,
            start_points,
        })
    }

    /// How many tick chunks the file holds.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Read tick chunk `index` into `buf` (reused between calls) and decode
    /// it. Chunks are decoded in order, from the first or from seat rows
    /// the decoder was given.
    pub fn read_chunk(
        &mut self,
        index: usize,
        decoder: &mut ChunkDecoder,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let &(offset, stored) = self
            .chunks
            .get(index)
            .context("film chunk is past the end")?;
        buf.clear();
        buf.resize(stored as usize, 0);
        read_exact_at(&mut self.input, offset, buf)?;
        decoder.load((index * CHUNK_TICKS) as u64 + 1, buf)
    }

    /// The stored frame of a machine envelope or start point.
    pub fn read_block(&mut self, block: &MachineBlock) -> Result<Vec<u8>> {
        let mut frame = vec![0u8; block.len as usize];
        read_exact_at(&mut self.input, block.offset, &mut frame)?;
        Ok(frame)
    }
}

/// The container's sections that a film carries: the machine part's boot
/// facts, startup decisions, initial disk, call tape, controller ports,
/// bot roster, hash checks and marks. A film holds no keyframe, and its
/// header goes in the seed.
pub fn carried(tag: Tag) -> bool {
    matches!(
        &tag,
        b"DATA" | b"BOOT" | b"MOVI" | b"TAPE" | b"PDEV" | b"BOTR" | b"HASH" | b"MARK"
    )
}

fn seats_of(tick: &blam::Tick) -> [Option<[u8; ACTION_BYTES]>; SEATS] {
    let mut seats = [None; SEATS];
    for input in &tick.inputs {
        if let Some(slot) = seats.get_mut(usize::from(input.seat)) {
            *slot = input.action;
        }
    }
    seats
}

/// What a conversion made.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Converted {
    pub ticks: u64,
    pub chunks: usize,
    pub machine_blocks: usize,
    /// Stored bytes of the container's tick blocks (inputs, poses and
    /// events together), and of the film file.
    pub container_tick_bytes: u64,
    pub film_bytes: u64,
}

/// Write the film file of a sealed container: its seats' actions and
/// digests as columns, its seed, and its machine part's envelopes copied
/// as stored.
pub fn from_container(container: &Path, film: &Path) -> Result<Converted> {
    let reader = Reader::open(container)?;
    let header = reader.header.clone();
    let mut track = TrackReader::new(Reader::open(container)?)?;
    let descriptor = track.descriptor.clone();
    let tps = u8::try_from(descriptor.ticks_per_second.unwrap_or(30)).unwrap_or(30);
    let seed = Seed { header, descriptor };
    let mut w = FilmWriter::new(
        File::create(film).with_context(|| film.display().to_string())?,
        &seed,
        tps,
    )?;
    while let Some(tick) = track.step()? {
        let mut flags = 0;
        if tick.post_hash.is_some() {
            flags |= TICK_DIGEST;
        }
        if !tick.events.is_empty() {
            flags |= TICK_EVENTS;
        }
        w.push_tick(flags, tick.post_hash.unwrap_or(0), &seats_of(&tick))?;
        for event in &tick.events {
            match event {
                Event::Join { player } => {
                    w.seat_record(player.seat, RECORD_JOIN, &[player.seat, player.kind as u8])?
                }
                Event::Leave { seat } | Event::Quit { seat } => {
                    w.seat_record(*seat, RECORD_LEAVE, &[*seat])?
                }
                _ => {}
            }
        }
    }
    let ticks = w.ticks();
    let mut source = File::open(container)?;
    let mut machine_blocks = 0;
    let entries: Vec<Entry> = reader
        .machine_section()
        .entries()
        .filter(|e| carried(e.tag))
        .cloned()
        .collect();
    for entry in entries {
        let mut frame = vec![0u8; entry.size as usize];
        read_exact_at(&mut source, entry.offset + 8, &mut frame)?;
        w.machine(entry.tag, &frame)?;
        machine_blocks += 1;
    }
    let mut file = w.finish()?;
    file.sync_all()?;
    let film_bytes = file.metadata()?.len();
    let container_tick_bytes = reader
        .entries
        .iter()
        .filter(|e| e.tag == blam::INPT || e.tag == blam::stream::TAG)
        .map(|e| u64::from(e.size) + 8)
        .sum();
    Ok(Converted {
        ticks,
        chunks: ticks.div_ceil(CHUNK_TICKS as u64) as usize,
        machine_blocks,
        container_tick_bytes,
        film_bytes,
    })
}

/// The film file's path beside its container.
pub fn beside(container: &Path) -> PathBuf {
    container.with_extension("h2f")
}

/// Write the film file beside a sealed container, and check it: every
/// tick's seats and actions decode to the container's, byte for byte, and
/// every digest's low 16 bits match.
pub fn write_beside(container: &Path) -> Result<Converted> {
    let film = beside(container);
    let converted = from_container(container, &film)?;
    verify(container, &film)?;
    Ok(converted)
}

/// Compare a film file with its container, tick by tick.
pub fn verify(container: &Path, film: &Path) -> Result<u64> {
    let mut reader = FilmReader::open(File::open(film)?)?;
    let mut track = TrackReader::open(container)?;
    let mut decoder = ChunkDecoder::new()?;
    let mut buf = Vec::new();
    let mut tick = 0u64;
    let mut chunk = 0;
    let mut seen = 0usize;
    while let Some(expected) = track.step()? {
        tick += 1;
        if seen == 0 {
            reader.read_chunk(chunk, &mut decoder, &mut buf)?;
            chunk += 1;
        }
        let t = seen;
        seen = (seen + 1) % CHUNK_TICKS;
        ensure!(
            t < decoder.ticks(),
            "film file has fewer ticks than its container (tick {tick})"
        );
        let seats = seats_of(&expected);
        let mut out = [0u8; ACTION_BYTES];
        for (s, action) in seats.iter().enumerate() {
            let present = decoder.action(t, s, &mut out);
            ensure!(
                present == action.is_some(),
                "tick {tick} seat {s}: presence differs"
            );
            if let Some(action) = action {
                ensure!(&out == action, "tick {tick} seat {s}: action differs");
            }
        }
        if let Some(hash) = expected.post_hash {
            ensure!(
                decoder.digest(t) == hash as u16,
                "tick {tick}: digest differs"
            );
        }
    }
    ensure!(
        tick == reader.length_ticks && chunk == reader.chunk_count(),
        "film file length differs from its container"
    );
    Ok(tick)
}

#[cfg(test)]
mod tests;
