//! The film file (docs/game/input-film.md, "Layout"): a seed, the seats'
//! action columns in chunks of 256 ticks ([`crate::columns`]), the game
//! start, and a trailer that finds a chunk without a search. A film is one
//! game: `<id>.h2film` is this file and `<id>.h2stats` beside it the stats
//! container ([`stats_of`]).
//!
//! ```text
//! header, 64 bytes, little-endian
//!    0   8  "H2FILM4!"
//!    8   u16 version (4), u16 flags (bit 0 unused, bit 1 sealed,
//!           bit 2 begins at a start point, bit 3 holds a game start,
//!           bit 4 holds a machine tape)
//!   12   u8 ticks per second, u8 seat limit (16), u16 ticks per chunk (256)
//!   16  16  film id (the first 16 bytes of the SHA-256 of its text id)
//!   32   u32 length in ticks, u32 stored bytes of the seed
//!   40   u64 offset of the trailer
//!   48  16  digest of the dictionary the chunks name, or zero
//! seed      one zstd frame of JSON: the film header and the match descriptor
//! blocks    u8 kind (0 tick chunk, 2 start point, 3 checkpoint, 4 game
//!           start, at most one, 5 extension), u32 stored bytes, then the
//!           payload: a tick chunk is its zstd frame; a start point and a
//!           game start ([`crate::start`]) are a zstd frame; an extension
//!           is a u32 update index, a four-byte feature tag and a zstd
//!           frame of the feature's bytes
//! trailer   u32 chunk count, then a u64 offset per tick chunk
//! ```
//!
//! The length, the trailer offset and the sealed flag are zero while a file
//! is written; a reader walks the blocks and stops at a torn last one.
//! Chunk i covers ticks 256 i + 1 to 256 i + 256. A tick's digest here is
//! the low 16 bits of the gameplay digest, and a chunk ends with the last
//! tick's whole digest.
use crate::{
    Header, Tag,
    blam::{MatchHeader, SeatInput},
    columns::{
        ACTION_BYTES, BODY_BYTES, CHUNK_TICKS, ChunkDecoder, ChunkEncoder, FRAME_CAP, SEATS, WORDS,
    },
    start::GameStart,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub const MAGIC: &[u8; 8] = b"H2FILM4!";
pub const VERSION: u16 = 4;
pub const HEADER_BYTES: usize = 64;
pub const FLAG_SEALED: u16 = 2;
pub const FLAG_START_POINT: u16 = 4;
/// The file holds its game's start ([`crate::start`]).
pub const FLAG_GAME_START: u16 = 8;
/// The file holds a client's machine tape ([`crate::tape`]).
pub const FLAG_TAPE: u16 = 16;
const KIND_TICKS: u8 = 0;
const KIND_START: u8 = 2;
const KIND_TAIL: u8 = 3;
const KIND_GAME: u8 = 4;
const KIND_EXTENSION: u8 = 5;
const KIND_TAPE: u8 = 6;
/// The most a stored tape block can take.
const TAPE_STORED: u64 = crate::tape::MAX_BLOCK_BYTES as u64 + (crate::tape::MAX_BLOCK_BYTES as u64 >> 7) + 1024;
/// The most bytes one extension record holds, inflated.
pub const MAX_EXTENSION_BYTES: usize = 1 << 20;
/// The most extension records a film holds.
pub const MAX_EXTENSIONS: usize = 1 << 16;
/// The most stored bytes the extension records of a film take in all.
pub const MAX_EXTENSION_TOTAL: u64 = 16 << 20;
/// The bytes before an extension's frame: its update index and its tag.
const EXTENSION_PREFIX: u32 = 8;
/// The most a stored extension frame can take: the cap's worst case for
/// zstd, with room for the frame's own header.
const EXTENSION_STORED: u64 = MAX_EXTENSION_BYTES as u64 + (MAX_EXTENSION_BYTES as u64 >> 7) + 1024;
const MAX_SEED: usize = 1 << 20;
const MAX_START: u32 = 4 << 20;
const MAX_GAME_START: u32 = 8 << 20;
/// Seat record kinds the recorder writes: a seat joined (its seat number
/// and kind), a seat left or quit (its seat number).
pub const RECORD_JOIN: u8 = 1;
pub const RECORD_LEAVE: u8 = 2;

/// Tick flags: the digest is present; events (in the stats stream) were
/// recorded on the tick.
pub const TICK_DIGEST: u8 = 1;
pub const TICK_EVENTS: u8 = 2;

/// Everything a player needs before the first tick besides the game
/// start: what the film's header says, and the match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seed {
    pub header: Header,
    pub descriptor: MatchHeader,
}

impl Seed {
    /// The seed as a film that leaves this machine carries it
    /// ([`share_copy`]): no network address, lease, local map path, host
    /// or account number.
    pub fn shared(&self) -> Seed {
        let mut seed = self.clone();
        seed.header.xnet_addr.clear();
        seed.header.lease_id = None;
        scrub(&mut seed.descriptor);
        seed
    }
}

/// Drop what names a machine or an account from a match descriptor.
fn scrub(descriptor: &mut MatchHeader) {
    descriptor.map.cache_path = None;
    descriptor.host_id = None;
    for seat in &mut descriptor.roster {
        seat.xuid = None;
    }
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
        ensure!(flags >> 5 == 0, "film file has unknown flags");
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
    extensions: usize,
    extension_bytes: u64,
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
            extensions: 0,
            extension_bytes: 0,
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
        ensure!(self.head.flags & FLAG_TAPE == 0, "a film takes no tick after its tape");
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

    /// The update's body for the tick just pushed, even the last of a
    /// chunk: stored as the bytes that differ from the tick before's. A
    /// tick given none holds the tick before's.
    pub fn tail(&mut self, body: &[u8]) -> Result<()> {
        self.encoder.tail(body)
    }

    /// A seat record on the tick just pushed, even the last of a chunk.
    pub fn seat_record(&mut self, seat: u8, kind: u8, bytes: &[u8]) -> Result<()> {
        let n = self.encoder.ticks();
        ensure!(n > 0, "a seat record needs a tick");
        self.encoder.seat_record(n - 1, seat, kind, bytes)
    }

    /// Write the open chunk as it stands, as a block a reader of an
    /// unsealed file takes as the file's last chunk until a whole chunk
    /// replaces it. A recording that dies keeps its ticks to here.
    pub fn checkpoint(&mut self) -> Result<()> {
        if self.encoder.ticks() == 0 {
            return Ok(());
        }
        let frame = self.encoder.finish_copy(self.last_digest)?;
        self.block(KIND_TAIL, &frame)?;
        self.out.flush()?;
        Ok(())
    }

    /// The game's start, before the game's first update: the film tick the
    /// start names is the next one this writer takes. A film holds one.
    pub fn game_start(&mut self, start: &GameStart) -> Result<()> {
        ensure!(
            start.game.first_update == self.ticks + 1,
            "a game start comes before its first update (tick {}, next is {})",
            start.game.first_update,
            self.ticks + 1
        );
        self.game_start_frame(&start.encode()?)
    }

    /// A game start as the file stores it, copied from another film file.
    pub fn game_start_frame(&mut self, frame: &[u8]) -> Result<()> {
        ensure!(
            self.head.flags & FLAG_GAME_START == 0,
            "a film holds one game start"
        );
        ensure!(
            frame.len() as u64 <= u64::from(MAX_GAME_START),
            "film game start is too large"
        );
        self.head.flags |= FLAG_GAME_START;
        self.block(KIND_GAME, frame)
    }

    /// A start point: the state a film that begins mid-game starts from.
    pub fn start_point(&mut self, frame: &[u8]) -> Result<()> {
        ensure!(self.ticks == 0, "a start point comes before the first tick");
        self.head.flags |= FLAG_START_POINT;
        self.block(KIND_START, frame)
    }

    /// A feature's outside input, applied at the start of `update`, which
    /// must be the next update this writer takes. The block precedes the
    /// chunk that holds the update: a full chunk the next tick would have
    /// closed is closed first. `tag` names the feature; the bytes are its
    /// own, at most [`MAX_EXTENSION_BYTES`]; a film holds at most
    /// [`MAX_EXTENSIONS`] and [`MAX_EXTENSION_TOTAL`] stored bytes of them.
    pub fn extension(&mut self, update: u32, tag: Tag, bytes: &[u8]) -> Result<()> {
        ensure!(
            u64::from(update) == self.ticks + 1,
            "an extension record comes before its update (update {update}, next is {})",
            self.ticks + 1
        );
        ensure!(
            bytes.len() <= MAX_EXTENSION_BYTES,
            "an extension record is larger than {MAX_EXTENSION_BYTES} bytes"
        );
        if self.encoder.ticks() == CHUNK_TICKS {
            self.close_chunk()?;
        }
        let frame = zstd::bulk::compress(bytes, 3)?;
        self.extension_frame(update, tag, &frame)
    }

    /// An extension as the file stores it, copied from another film file.
    fn extension_frame(&mut self, update: u32, tag: Tag, frame: &[u8]) -> Result<()> {
        let stored = u64::from(EXTENSION_PREFIX) + frame.len() as u64;
        ensure!(
            self.extensions < MAX_EXTENSIONS,
            "a film holds at most {MAX_EXTENSIONS} extension records"
        );
        ensure!(
            self.extension_bytes + stored <= MAX_EXTENSION_TOTAL,
            "a film's extension records take at most {MAX_EXTENSION_TOTAL} bytes"
        );
        let mut payload = Vec::with_capacity(stored as usize);
        payload.extend_from_slice(&update.to_le_bytes());
        payload.extend_from_slice(&tag);
        payload.extend_from_slice(frame);
        self.block(KIND_EXTENSION, &payload)?;
        self.extensions += 1;
        self.extension_bytes += stored;
        Ok(())
    }

    /// A client's machine tape ([`crate::tape`]), as blocks after the
    /// ticks written so far, which closes the last chunk: the film takes no
    /// tick after it. The recorder writes a client game's tape once, when
    /// it seals the game.
    pub fn tape(&mut self, tape: &crate::tape::Tape) -> Result<()> {
        if tape.is_empty() {
            return Ok(());
        }
        if self.encoder.ticks() > 0 {
            self.close_chunk()?;
        }
        for block in tape.blocks()? {
            let frame = zstd::bulk::compress(&block, 3)?;
            self.block(KIND_TAPE, &frame)?;
        }
        self.head.flags |= FLAG_TAPE;
        Ok(())
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

/// A start point or game start in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub tag: Tag,
    /// Where the stored frame starts, and its length.
    pub offset: u64,
    pub len: u32,
}

/// One extension record of the file: the update it comes before, its
/// feature and where its stored frame is ([`FilmReader::read_extension`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionBlock {
    pub update: u32,
    pub tag: Tag,
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
    pub start_points: Vec<Block>,
    /// The game's start, when the film holds one
    /// ([`FilmReader::game_start`]); a file with two is refused.
    pub game_starts: Vec<Block>,
    /// The extension records, in update order.
    pub extensions: Vec<ExtensionBlock>,
    /// The machine tape's blocks, in file order: (offset of the stored
    /// frame, its length) ([`FilmReader::tape`]).
    pub tape_blocks: Vec<(u64, u32)>,
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
        let mut start_points = Vec::new();
        let mut game_starts = Vec::new();
        let mut extensions: Vec<ExtensionBlock> = Vec::new();
        let mut extension_bytes = 0u64;
        let mut tape_blocks = Vec::new();
        let mut tail = None;
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
                    tail = None;
                }
                KIND_TAIL => {
                    ensure!(
                        stored as usize <= FRAME_CAP,
                        "film tick chunk is larger than a chunk can be"
                    );
                    tail = Some((body, stored));
                }
                KIND_START => {
                    ensure!(stored <= MAX_START, "film start point is too large");
                    start_points.push(Block {
                        tag: *b"STRT",
                        offset: body,
                        len: stored,
                    });
                }
                KIND_GAME => {
                    ensure!(stored <= MAX_GAME_START, "film game start is too large");
                    ensure!(game_starts.is_empty(), "film holds a second game start");
                    game_starts.push(Block {
                        tag: *b"GAME",
                        offset: body,
                        len: stored,
                    });
                }
                KIND_EXTENSION => {
                    ensure!(
                        stored >= EXTENSION_PREFIX
                            && u64::from(stored) <= u64::from(EXTENSION_PREFIX) + EXTENSION_STORED,
                        "film extension record is cut short or too large"
                    );
                    ensure!(
                        extensions.len() < MAX_EXTENSIONS,
                        "film holds more than {MAX_EXTENSIONS} extension records"
                    );
                    extension_bytes += u64::from(stored);
                    ensure!(
                        extension_bytes <= MAX_EXTENSION_TOTAL,
                        "film extension records are larger than {MAX_EXTENSION_TOTAL} bytes"
                    );
                    let mut prefix = [0u8; EXTENSION_PREFIX as usize];
                    read_exact_at(&mut input, body, &mut prefix)?;
                    let update = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
                    let first = chunks.len() as u64 * CHUNK_TICKS as u64;
                    ensure!(
                        u64::from(update) > first
                            && u64::from(update) <= first + CHUNK_TICKS as u64,
                        "film extension record is not before the chunk of its update"
                    );
                    ensure!(
                        extensions.last().is_none_or(|last| last.update <= update),
                        "film extension records are out of order"
                    );
                    extensions.push(ExtensionBlock {
                        update,
                        tag: [prefix[4], prefix[5], prefix[6], prefix[7]],
                        offset: body + u64::from(EXTENSION_PREFIX),
                        len: stored - EXTENSION_PREFIX,
                    });
                }
                KIND_TAPE => {
                    ensure!(u64::from(stored) <= TAPE_STORED, "film tape block is too large");
                    tape_blocks.push((body, stored));
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
            let whole = (chunks.len() * CHUNK_TICKS) as u64;
            match tail {
                Some((body, stored)) => {
                    let mut frame = vec![0u8; stored as usize];
                    read_exact_at(&mut input, body, &mut frame)?;
                    let raw = zstd::bulk::decompress(&frame, 4 << 20)
                        .context("film checkpoint does not inflate")?;
                    ensure!(raw.len() >= 2, "film checkpoint is cut short");
                    let n = u64::from(u16::from_le_bytes([raw[0], raw[1]]));
                    ensure!(
                        (1..=CHUNK_TICKS as u64).contains(&n),
                        "film checkpoint holds a bad tick count"
                    );
                    chunks.push((body, stored));
                    whole + n
                }
                None => whole,
            }
        };
        Ok(Self {
            input,
            seed,
            flags: head.flags,
            ticks_per_second: head.ticks_per_second,
            sealed,
            length_ticks,
            chunks,
            start_points,
            game_starts,
            extensions,
            tape_blocks,
        })
    }

    /// Whose game the film is ([`crate::tape::Role`]).
    pub fn role(&self) -> crate::tape::Role {
        self.seed.header.role
    }

    /// The machine tape, both streams from index 0; empty for a film with
    /// none (every film that is not a client's).
    pub fn tape(&mut self) -> Result<crate::tape::Tape> {
        let mut tape = crate::tape::Tape::default();
        for (index, &(offset, stored)) in self.tape_blocks.clone().iter().enumerate() {
            let mut frame = vec![0u8; stored as usize];
            read_exact_at(&mut self.input, offset, &mut frame)?;
            let block = zstd::bulk::decompress(&frame, crate::tape::MAX_BLOCK_BYTES)
                .with_context(|| format!("film tape block {index} does not inflate"))?;
            tape.push_block(&block)
                .with_context(|| format!("film tape block {index}"))?;
        }
        Ok(tape)
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

    /// The film's game start. `None` for a film that holds none.
    pub fn game_start(&mut self) -> Result<Option<GameStart>> {
        let Some(block) = self.game_starts.first().copied() else {
            return Ok(None);
        };
        self.game_start_at(&block).map(Some)
    }

    /// One game start of the file, decoded.
    pub fn game_start_at(&mut self, block: &Block) -> Result<GameStart> {
        let frame = self.read_block(block)?;
        GameStart::decode(&frame)
    }

    /// The feature bytes of an extension record, inflated.
    pub fn read_extension(&mut self, block: &ExtensionBlock) -> Result<Vec<u8>> {
        let mut frame = vec![0u8; block.len as usize];
        read_exact_at(&mut self.input, block.offset, &mut frame)?;
        zstd::bulk::decompress(&frame, MAX_EXTENSION_BYTES)
            .context("film extension record does not inflate")
    }

    /// The stored frame of a game start or start point.
    pub fn read_block(&mut self, block: &Block) -> Result<Vec<u8>> {
        let mut frame = vec![0u8; block.len as usize];
        read_exact_at(&mut self.input, block.offset, &mut frame)?;
        Ok(frame)
    }
}

/// What a film file is read from.
pub trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

/// One tick of a film file as a player plays it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickRow {
    /// The seats that acted, with no pad.
    pub inputs: Vec<SeatInput>,
    /// The low 16 bits of the tick's gameplay digest, if it has one.
    pub digest: Option<u16>,
    /// The whole digest, on the digested tick that closes a chunk.
    pub closing: Option<u64>,
    /// The update's body between its action block and its stamp
    /// ([`BODY_BYTES`] long).
    pub body: Vec<u8>,
}

type Rows = ([[u32; WORDS]; SEATS], [[u32; WORDS]; SEATS]);

/// A film file's actions by tick, for a [`crate::blam::TrackReader`] that takes its
/// inputs from the film ([`crate::blam::TrackReader::attach_film`]). Ticks are asked for
/// in any order: a chunk is decoded from the nearest predictor state seen,
/// and each chunk's closing state is kept as chunks are decoded.
pub struct FilmActions {
    reader: FilmReader<Box<dyn Source>>,
    decoder: ChunkDecoder,
    buf: Vec<u8>,
    loaded: Option<usize>,
    /// The decoder state before chunk i, once chunk i - 1 has been decoded.
    starts: Vec<Option<Rows>>,
}

impl FilmActions {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| path.display().to_string())?;
        Self::new(Box::new(file))
    }

    pub fn new(input: Box<dyn Source>) -> Result<Self> {
        let reader = FilmReader::open(input)?;
        let starts = vec![None; reader.chunk_count() + 1];
        Ok(Self {
            reader,
            decoder: ChunkDecoder::new()?,
            buf: Vec::new(),
            loaded: None,
            starts,
        })
    }

    pub fn seed(&self) -> &Seed {
        &self.reader.seed
    }

    pub fn length_ticks(&self) -> u64 {
        self.reader.length_ticks
    }

    /// The features the film's extension records are for, each once.
    pub fn extension_tags(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = Vec::new();
        for ext in &self.reader.extensions {
            if !tags.contains(&ext.tag) {
                tags.push(ext.tag);
            }
        }
        tags
    }

    /// The extension records that come before `update`, in the order the
    /// recording wrote them: each one's feature and bytes.
    pub fn extensions_at(&mut self, update: u64) -> Result<Vec<(Tag, Vec<u8>)>> {
        let from = self
            .reader
            .extensions
            .partition_point(|e| u64::from(e.update) < update);
        let to = self
            .reader
            .extensions
            .partition_point(|e| u64::from(e.update) <= update);
        let blocks = self.reader.extensions[from..to].to_vec();
        blocks
            .iter()
            .map(|block| Ok((block.tag, self.reader.read_extension(block)?)))
            .collect()
    }

    fn load(&mut self, k: usize) -> Result<()> {
        if self.loaded == Some(k) {
            return Ok(());
        }
        let from = if self.loaded.map(|l| l + 1) == Some(k) {
            k
        } else {
            let j = (1..=k)
                .rev()
                .find(|&j| self.starts.get(j).is_some_and(Option::is_some))
                .unwrap_or(0);
            match self.starts.get(j).copied().flatten() {
                Some((p1, p2)) if j > 0 => self.decoder.set_seat_rows(p1, p2),
                _ => self.decoder.reset(),
            }
            j
        };
        self.loaded = None;
        for i in from..=k {
            self.reader
                .read_chunk(i, &mut self.decoder, &mut self.buf)?;
            self.starts[i + 1] = Some(self.decoder.seat_rows());
        }
        self.loaded = Some(k);
        Ok(())
    }

    /// The film's game start ([`FilmReader::game_start`]).
    pub fn game_start(&mut self) -> Result<Option<GameStart>> {
        self.reader.game_start()
    }

    /// Load the chunk holding `tick` and give the tick's index in it.
    fn locate(&mut self, tick: u64) -> Result<usize> {
        ensure!(tick >= 1, "film ticks start at 1");
        let k = ((tick - 1) / CHUNK_TICKS as u64) as usize;
        let t = ((tick - 1) % CHUNK_TICKS as u64) as usize;
        self.load(k)
            .with_context(|| format!("film file has no chunk for tick {tick}"))?;
        ensure!(
            t < self.decoder.ticks(),
            "film file ends before tick {tick}"
        );
        Ok(t)
    }

    /// What the film holds for `tick`: each seat's action, the low 16 bits
    /// of the tick's gameplay digest when it has one, and, when the tick
    /// closes its chunk, the whole digest the chunk ends with. A player
    /// that plays the film from its game start writes the actions into the
    /// game and checks the digests against it ([`TickRow`]).
    pub fn row(&mut self, tick: u64) -> Result<TickRow> {
        let t = self.locate(tick)?;
        let mut inputs = Vec::new();
        for seat in 0..SEATS {
            let mut action = [0u8; ACTION_BYTES];
            if self.decoder.action(t, seat, &mut action) {
                inputs.push(SeatInput {
                    seat: seat as u8,
                    pad: None,
                    action: Some(action),
                });
            }
        }
        let mut body = [0u8; BODY_BYTES];
        ensure!(
            self.decoder.body(t, &mut body),
            "film file has no update body for tick {tick}"
        );
        let digested = self.decoder.flags(t) & TICK_DIGEST != 0;
        Ok(TickRow {
            body: body.to_vec(),
            inputs,
            digest: digested.then(|| self.decoder.digest(t)),
            closing: (digested && t + 1 == self.decoder.ticks()).then(|| self.decoder.arena_hash()),
        })
    }

    /// Put the film's actions on `tick` in `inputs`. A seat's pad, which
    /// the film does not carry, stays as `inputs` had it. If the stats
    /// stream has the tick's gameplay digest, the film's low 16 bits must
    /// agree with it.
    pub fn fill(
        &mut self,
        tick: u64,
        inputs: &mut Vec<SeatInput>,
        digest: Option<u64>,
    ) -> Result<()> {
        let t = self.locate(tick)?;
        if let Some(digest) = digest
            && self.decoder.flags(t) & TICK_DIGEST != 0
        {
            ensure!(
                self.decoder.digest(t) == digest as u16,
                "film file and stats stream disagree at tick {tick}"
            );
        }
        let old = std::mem::take(inputs);
        for seat in 0..SEATS {
            let mut action = [0u8; ACTION_BYTES];
            let present = self.decoder.action(t, seat, &mut action);
            let pad = old
                .iter()
                .find(|input| usize::from(input.seat) == seat)
                .and_then(|input| input.pad);
            if present || pad.is_some() {
                inputs.push(SeatInput {
                    seat: seat as u8,
                    pad,
                    action: present.then_some(action),
                });
            }
        }
        Ok(())
    }
}

/// The game start of the film file at `film`, or `None` when there is no
/// such file or it holds no game start. Reads the file's index and the one
/// block; no tick chunk is inflated.
pub fn game_start_of(film: &Path) -> Result<Option<GameStart>> {
    if !film.try_exists()? {
        return Ok(None);
    }
    let file = File::open(film).with_context(|| film.display().to_string())?;
    FilmReader::open(file)?.game_start()
}

/// The film's header as its seed holds it, with the length the file reads
/// to (sealed, or up to its last checkpoint) and the simulation hooks of
/// its game start's stamp: a recording that opened its film before the
/// hooks were installed names them there. An unsealed film reads too.
pub fn header_of(film: &Path) -> Result<Header> {
    let file = File::open(film).with_context(|| film.display().to_string())?;
    let mut reader = FilmReader::open(file)?;
    let start = reader.game_start()?;
    let mut header = reader.seed.header.clone();
    header.meta.length_ticks = reader.length_ticks;
    if let Some(hooks) = start.and_then(|start| start.stamp.hooks_sha256) {
        header.simulation_hooks = Some(hooks);
    }
    Ok(header)
}

/// The stats container of the film file at `film`: `<id>.h2stats` beside
/// `<id>.h2film` (docs/game/film-format.md).
pub fn stats_of(film: &Path) -> PathBuf {
    film.with_extension("h2stats")
}

/// Take the recorder's hold on a film file it is about to write: an
/// exclusive lock on its open file, kept until the file is closed, which a
/// process exit, a crash or a kill releases ([`being_written`]).
pub fn hold(file: &File) -> Result<()> {
    file.try_lock()
        .map_err(|e| anyhow::anyhow!("film file is held by another recording: {e}"))
}

/// Whether a recorder holds the film file at `film` ([`hold`]): an unsealed
/// film it holds is a game in progress, kept out of the library's listing
/// and left alone by its prune. One no process holds is a recording that
/// stopped without sealing.
pub fn being_written(film: &Path) -> bool {
    let Ok(file) = File::open(film) else {
        return false;
    };
    matches!(
        file.try_lock_shared(),
        Err(std::fs::TryLockError::WouldBlock)
    )
}

/// The copy of the sealed film at `film` that goes to the account's film
/// library on Live (`live-hle` `films::sync`): its start points and tick
/// chunks byte for byte, its game start with the descriptor scrubbed, and
/// the seed of [`Seed::shared`]. Checkpoint blocks are left out. Live
/// checks the copy's seed (`live-server` `films/shared/verify.rs`).
pub fn share_copy<W: Write + Seek>(film: &Path, out: W) -> Result<W> {
    let mut reader = FilmReader::open(File::open(film)?)?;
    ensure!(reader.sealed, "only a sealed film is shared");
    ensure!(
        reader.tape_blocks.is_empty(),
        "a client film is not shared: its tape holds this machine's network traffic"
    );
    let mut copy = FilmWriter::new(out, &reader.seed.shared(), reader.ticks_per_second)?;
    for block in reader.start_points.clone() {
        let frame = reader.read_block(&block)?;
        copy.start_point(&frame)?;
    }
    if let Some(block) = reader.game_starts.first().copied() {
        let mut start = GameStart::decode(&reader.read_block(&block)?)?;
        scrub(&mut start.game.descriptor);
        copy.game_start_frame(&start.encode()?)?;
    }
    for (index, (body, stored)) in reader.chunks.clone().into_iter().enumerate() {
        let last = ((index + 1) * CHUNK_TICKS) as u64;
        for ext in reader.extensions.clone() {
            if u64::from(ext.update) > last - CHUNK_TICKS as u64 && u64::from(ext.update) <= last {
                let mut frame = vec![0u8; ext.len as usize];
                read_exact_at(&mut reader.input, ext.offset, &mut frame)?;
                copy.extension_frame(ext.update, ext.tag, &frame)?;
            }
        }
        let mut frame = vec![0u8; stored as usize];
        read_exact_at(&mut reader.input, body, &mut frame)?;
        let at = copy.out.stream_position()?;
        copy.block(KIND_TICKS, &frame)?;
        copy.chunks.push(at);
    }
    copy.ticks = reader.length_ticks;
    copy.finish()
}

/// Seal the unsealed film file at `film`, ending at `end_tick`: a sealed
/// copy of its start, its ticks up to `end_tick` and their seat records,
/// renamed over it once it is whole. A hard-killed recording leaves its
/// film file as the recorder last wrote it, with the open chunk as of the
/// last [`FilmWriter::checkpoint`].
pub fn recover(film: &Path, end_tick: u64) -> Result<()> {
    let staging = film.with_extension("h2film.recover");
    let sealed = (|| -> Result<File> {
        let mut reader = FilmReader::open(File::open(film)?)?;
        ensure!(
            reader.length_ticks >= end_tick,
            "film file ends at tick {}, before tick {end_tick}",
            reader.length_ticks
        );
        let mut out = FilmWriter::new(
            File::create(&staging)?,
            &reader.seed,
            reader.ticks_per_second,
        )?;
        for block in reader.start_points.clone() {
            let frame = reader.read_block(&block)?;
            out.start_point(&frame)?;
        }
        if let Some(block) = reader.game_starts.first().copied() {
            let frame = reader.read_block(&block)?;
            out.game_start_frame(&frame)?;
        }
        let mut decoder = ChunkDecoder::new()?;
        let mut buf = Vec::new();
        'chunks: for chunk in 0..reader.chunk_count() {
            reader.read_chunk(chunk, &mut decoder, &mut buf)?;
            for t in 0..decoder.ticks() {
                let tick = (chunk * CHUNK_TICKS + t) as u64 + 1;
                if tick > end_tick {
                    break 'chunks;
                }
                for ext in reader
                    .extensions
                    .clone()
                    .iter()
                    .filter(|e| u64::from(e.update) == tick)
                {
                    let bytes = reader.read_extension(ext)?;
                    out.extension(ext.update, ext.tag, &bytes)?;
                }
                let mut seats = [None; SEATS];
                for (seat, slot) in seats.iter_mut().enumerate() {
                    let mut action = [0u8; ACTION_BYTES];
                    if decoder.action(t, seat, &mut action) {
                        *slot = Some(action);
                    }
                }
                let digest = if t + 1 == decoder.ticks() {
                    decoder.arena_hash()
                } else {
                    u64::from(decoder.digest(t))
                };
                out.push_tick(decoder.flags(t), digest, &seats)?;
                let mut body = [0u8; BODY_BYTES];
                ensure!(
                    decoder.body(t, &mut body),
                    "film chunk has no body for tick {tick}"
                );
                out.tail(&body)?;
                for record in decoder.seat_records().filter(|r| usize::from(r.tick) == t) {
                    out.seat_record(record.seat, record.kind, record.bytes)?;
                }
            }
        }
        ensure!(
            out.ticks() == end_tick,
            "film file is short of tick {end_tick}"
        );
        let file = out.finish()?;
        file.sync_all()?;
        Ok(file)
    })();
    match sealed {
        Ok(_) => Ok(std::fs::rename(&staging, film)?),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
