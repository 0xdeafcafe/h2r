//! The film's action columns (docs/game/input-film.md, "The encoding").
//!
//! A chunk is up to 256 ticks. For each seat that is in the game on any of
//! them, the 92-byte retail action is split into its fields, each field
//! into 32-bit words, and each word into the residual of its value against
//! a prediction, one byte plane at a time. The whole chunk is one zstd
//! frame, so a chunk is a seek unit that needs nothing before it but the
//! last two values of each seat's words, which the decoder holds.
//!
//! ```text
//! u16     ticks in the chunk (1 through 256), u16 mask of seats that are in
//!         the game on at least one of them
//! n       tick flags, one byte a tick
//! n, n    the low byte, then the high byte, of the low 16 bits of the
//!         tick's gameplay digest
//! per seat in the mask, ascending:
//!   u16   field mask: bit 0 the seat's presence (absent: in the game on
//!         every tick), bit i + 1 the action field i (absent: the value
//!         the chunk before ended on, on every tick)
//!   bit 0 set: n bytes, 0 or 1 a tick
//!   per field in the mask, ascending:
//!     u8  predictor (0 none, 1 previous, 2 linear, 3 grid)
//!     u32 for a grid: the denominator
//!     per word of the field, four planes of n bytes (the residual's bytes,
//!     lowest first)
//!     for a grid: u8 count, then each value that is off the grid as a
//!     u8 tick and its u32 bits
//! u8      seat record count; each: u8 tick, u8 seat, u8 kind, u16 length,
//!         bytes (at most 8,192 in all)
//! u64     the arena hash after the chunk's last tick
//! ```
//!
//! A residual is the value minus its prediction as a 32-bit integer,
//! zigzag-coded: `none` predicts zero (the bits as they are), `previous`
//! the last tick's value, `linear` twice the last tick's value minus the
//! one before it. `grid` is for a one-word field of `f32` that sits on a
//! grid of 1/den: the residual is the step in grid units from the last
//! value's nearest step, and a value that is not exactly its step's
//! float comes whole in the exception list. A seat that is not in the game
//! on a tick keeps its last value, so its columns see no change.
//!
//! The encoder and the decoder own every buffer they use and allocate
//! nothing after `new`.
use anyhow::{Context, Result, bail, ensure};

pub const SEATS: usize = 16;
pub const ACTION_BYTES: usize = 92;
pub const WORDS: usize = ACTION_BYTES / 4;
pub const CHUNK_TICKS: usize = 256;
pub const RECORD_BYTES: usize = 8192;
/// The most seat records in a chunk.
pub const RECORD_LIMIT: usize = 255;
const GRID_MAX: u32 = 1 << 20;
/// A bound on one inflated chunk: the header, flags, digests, every seat
/// with every field present and every residual plane, 255 exceptions a
/// grid field, the seat records and the arena hash.
const RAW_CAP: usize = 4
    + 3 * CHUNK_TICKS
    + SEATS * (2 + CHUNK_TICKS + 13 * (1 + 4) + WORDS * 4 * CHUNK_TICKS + 13 * (1 + 5 * 64))
    + 1
    + RECORD_LIMIT * 6
    + RECORD_BYTES
    + 8;
pub const FRAME_CAP: usize = 192 << 10;
/// The grids the encoder tries: the sizes of an 8-bit, 10-bit and 16-bit
/// axis or pressure and the powers of two beside them.
pub const GRIDS: [u32; 12] = [
    63, 64, 127, 128, 255, 256, 1023, 1024, 4095, 4096, 32767, 65535,
];

/// One field of the 92-byte action.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    /// The first 32-bit word, and how many.
    pub first: usize,
    pub words: usize,
}

pub const FIELDS: [Field; 13] = [
    Field {
        name: "buttons",
        first: 0,
        words: 1,
    },
    Field {
        name: "yaw",
        first: 1,
        words: 1,
    },
    Field {
        name: "pitch",
        first: 2,
        words: 1,
    },
    Field {
        name: "throttle_x",
        first: 3,
        words: 1,
    },
    Field {
        name: "throttle_y",
        first: 4,
        words: 1,
    },
    Field {
        name: "trigger_primary",
        first: 5,
        words: 1,
    },
    Field {
        name: "trigger_secondary",
        first: 6,
        words: 1,
    },
    Field {
        name: "hold",
        first: 7,
        words: 1,
    },
    Field {
        name: "weapons",
        first: 8,
        words: 1,
    },
    Field {
        name: "zoom",
        first: 9,
        words: 1,
    },
    Field {
        name: "interaction",
        first: 10,
        words: 3,
    },
    Field {
        name: "assist",
        first: 13,
        words: 9,
    },
    Field {
        name: "tail",
        first: 22,
        words: 1,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predictor {
    None = 0,
    Previous = 1,
    Linear = 2,
    Grid = 3,
}

impl Predictor {
    fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => Self::None,
            1 => Self::Previous,
            2 => Self::Linear,
            3 => Self::Grid,
            _ => bail!("unknown predictor {b}"),
        })
    }
}

fn zig(d: u32) -> u32 {
    let s = d as i32;
    ((s << 1) ^ (s >> 31)) as u32
}

fn unzig(z: u32) -> u32 {
    ((z >> 1) as i32 ^ -((z & 1) as i32)) as u32
}

/// The grid step nearest a value. A cast saturates and sends NaN to zero,
/// so every bit pattern has one.
fn step_of(bits: u32, den: u32) -> i32 {
    (f32::from_bits(bits) * den as f32).round() as i32
}

fn from_step(step: i32, den: u32) -> u32 {
    (step as f32 / den as f32).to_bits()
}

fn words_of(action: &[u8; ACTION_BYTES], out: &mut [u32]) {
    for (word, bytes) in out.iter_mut().zip(action.chunks_exact(4)) {
        *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
}

/// Values the grid cannot say, by tick.
struct Exceptions {
    count: usize,
    tick: [u8; CHUNK_TICKS],
    bits: [u32; CHUNK_TICKS],
}

impl Exceptions {
    fn new() -> Self {
        Self {
            count: 0,
            tick: [0; CHUNK_TICKS],
            bits: [0; CHUNK_TICKS],
        }
    }
}

/// One seat's words for the ticks of a chunk, tick by tick, and the two
/// values before the chunk.
struct Column<'a> {
    values: &'a [u32],
    n: usize,
    p1: &'a [u32; WORDS],
    p2: &'a [u32; WORDS],
}

impl Column<'_> {
    fn word(&self, t: usize, w: usize) -> u32 {
        self.values[t * WORDS + w]
    }

    /// Whether no word of the field leaves the value the chunk before ended on.
    fn holds(&self, field: &Field) -> bool {
        (0..self.n).all(|t| {
            (field.first..field.first + field.words).all(|w| self.word(t, w) == self.p1[w])
        })
    }

    /// One word's residuals under a predictor, into `out[..n]`, and the
    /// values a grid cannot say.
    fn residuals(
        &self,
        w: usize,
        pred: Predictor,
        den: u32,
        out: &mut [u32],
        exc: &mut Exceptions,
    ) {
        let (mut a, mut b) = (self.p1[w], self.p2[w]);
        for t in 0..self.n {
            let v = self.word(t, w);
            out[t] = match pred {
                Predictor::None => v,
                Predictor::Previous => zig(v.wrapping_sub(a)),
                Predictor::Linear => zig(v.wrapping_sub(a.wrapping_mul(2).wrapping_sub(b))),
                Predictor::Grid => {
                    let step = step_of(v, den);
                    if from_step(step, den) != v && exc.count < CHUNK_TICKS {
                        exc.tick[exc.count] = t as u8;
                        exc.bits[exc.count] = v;
                        exc.count += 1;
                    }
                    zig(step.wrapping_sub(step_of(a, den)) as u32)
                }
            };
            b = a;
            a = v;
        }
    }

    /// The planes of a field under a predictor, `words * 4 * n` bytes.
    fn planes(
        &self,
        field: &Field,
        pred: Predictor,
        den: u32,
        res: &mut [u32],
        planes: &mut [u8],
        exc: &mut Exceptions,
    ) {
        exc.count = 0;
        let n = self.n;
        for k in 0..field.words {
            self.residuals(field.first + k, pred, den, res, exc);
            for t in 0..n {
                let r = res[t].to_le_bytes();
                for (j, byte) in r.iter().enumerate() {
                    planes[(k * 4 + j) * n + t] = *byte;
                }
            }
        }
    }
}

/// A guess at what a coder will make of planes: the bytes that are not zero
/// and the bytes that differ from the one before them, plane by plane.
fn cost(planes: &[u8], n: usize) -> usize {
    let mut total = 0;
    for plane in planes.chunks_exact(n) {
        let mut last = 0;
        for &b in plane {
            total += usize::from(b != 0) + usize::from(b != last);
            last = b;
        }
    }
    total
}

/// What the encoder keeps for one field of one seat after choosing.
struct Choice {
    pred: Predictor,
    den: u32,
    cost: usize,
}

/// Every predictor for a field, the cheapest by [`cost`]; its planes are
/// left in `best`, its exceptions in `best_exc`.
struct Scratch {
    res: Box<[u32]>,
    planes: Box<[u8]>,
    best: Box<[u8]>,
    exc: Exceptions,
    best_exc: Exceptions,
}

impl Scratch {
    fn new() -> Self {
        let plane = 9 * 4 * CHUNK_TICKS;
        Self {
            res: vec![0; CHUNK_TICKS].into(),
            planes: vec![0; plane].into(),
            best: vec![0; plane].into(),
            exc: Exceptions::new(),
            best_exc: Exceptions::new(),
        }
    }

    fn choose(&mut self, column: &Column, field: &Field) -> Choice {
        let n = column.n;
        let size = field.words * 4 * n;
        let mut best = Choice {
            pred: Predictor::None,
            den: 0,
            cost: usize::MAX,
        };
        let mut candidates: [(Predictor, u32); 3 + GRIDS.len()] =
            [(Predictor::None, 0); 3 + GRIDS.len()];
        candidates[1] = (Predictor::Previous, 0);
        candidates[2] = (Predictor::Linear, 0);
        let mut count = 3;
        if field.words == 1 {
            for den in GRIDS {
                candidates[count] = (Predictor::Grid, den);
                count += 1;
            }
        }
        for &(pred, den) in &candidates[..count] {
            column.planes(
                field,
                pred,
                den,
                &mut self.res,
                &mut self.planes[..size],
                &mut self.exc,
            );
            if pred == Predictor::Grid && self.exc.count * 4 > n {
                continue;
            }
            let c = cost(&self.planes[..size], n) + self.exc.count * 5;
            if c < best.cost {
                best = Choice { pred, den, cost: c };
                self.best[..size].copy_from_slice(&self.planes[..size]);
                self.best_exc.count = self.exc.count;
                self.best_exc.tick = self.exc.tick;
                self.best_exc.bits = self.exc.bits;
            }
        }
        best
    }
}

/// A buffer that never grows.
struct Out {
    buf: Box<[u8]>,
    len: usize,
}

impl Out {
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        let end = self.len + bytes.len();
        ensure!(
            end <= self.buf.len(),
            "film chunk is larger than a chunk can be"
        );
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}

/// The two values before the next tick, for every seat's every word.
#[derive(Clone, Copy)]
struct State {
    p1: [[u32; WORDS]; SEATS],
    p2: [[u32; WORDS]; SEATS],
}

impl State {
    const ZERO: Self = Self {
        p1: [[0; WORDS]; SEATS],
        p2: [[0; WORDS]; SEATS],
    };

    /// The state after `n` ticks of `values`, which hold every seat.
    fn advance(&mut self, values: &[u32], n: usize) {
        for s in 0..SEATS {
            let seat = &values[s * CHUNK_TICKS * WORDS..];
            for w in 0..WORDS {
                let last = seat[(n - 1) * WORDS + w];
                let before = if n >= 2 {
                    seat[(n - 2) * WORDS + w]
                } else {
                    self.p1[s][w]
                };
                self.p1[s][w] = last;
                self.p2[s][w] = before;
            }
        }
    }
}

/// A seat record the encoder was given.
pub struct SeatRecord<'a> {
    pub tick: u8,
    pub seat: u8,
    pub kind: u8,
    pub bytes: &'a [u8],
}

pub struct ChunkEncoder {
    n: usize,
    flags: Box<[u8]>,
    digests: Box<[u16]>,
    present: Box<[u8]>,
    values: Box<[u32]>,
    state: State,
    records: Out,
    record_count: usize,
    record_payload: usize,
    raw: Out,
    scratch: Scratch,
    frame: Box<[u8]>,
    zstd: zstd::bulk::Compressor<'static>,
}

impl ChunkEncoder {
    pub fn new() -> Result<Self> {
        let mut zstd = zstd::bulk::Compressor::new(3)?;
        zstd.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))?;
        Ok(Self {
            n: 0,
            flags: vec![0; CHUNK_TICKS].into(),
            digests: vec![0; CHUNK_TICKS].into(),
            present: vec![0; SEATS * CHUNK_TICKS].into(),
            values: vec![0; SEATS * CHUNK_TICKS * WORDS].into(),
            state: State::ZERO,
            records: Out {
                buf: vec![0; RECORD_BYTES + RECORD_LIMIT * 6].into(),
                len: 0,
            },
            record_count: 0,
            record_payload: 0,
            raw: Out {
                buf: vec![0; RAW_CAP].into(),
                len: 0,
            },
            scratch: Scratch::new(),
            frame: vec![0; FRAME_CAP].into(),
            zstd,
        })
    }

    pub fn ticks(&self) -> usize {
        self.n
    }

    /// The inflated chunk last finished, for the tests that corrupt it.
    #[cfg(test)]
    fn raw(&self) -> &[u8] {
        &self.raw.buf[..self.raw.len]
    }

    /// One tick: its flags, its gameplay digest and each seat's action, or
    /// none for a seat that is not in the game.
    pub fn push_tick(
        &mut self,
        flags: u8,
        digest: u64,
        seats: &[Option<[u8; ACTION_BYTES]>; SEATS],
    ) -> Result<()> {
        ensure!(self.n < CHUNK_TICKS, "film chunk is full");
        let t = self.n;
        self.flags[t] = flags;
        self.digests[t] = digest as u16;
        for (s, action) in seats.iter().enumerate() {
            let at = (s * CHUNK_TICKS + t) * WORDS;
            self.present[s * CHUNK_TICKS + t] = u8::from(action.is_some());
            match action {
                Some(action) => words_of(action, &mut self.values[at..at + WORDS]),
                None if t == 0 => self.values[at..at + WORDS].copy_from_slice(&self.state.p1[s]),
                None => self.values.copy_within(at - WORDS..at, at),
            }
        }
        self.n += 1;
        Ok(())
    }

    /// A seat record on the tick at `tick` in this chunk (its offset, not
    /// its number).
    pub fn seat_record(&mut self, tick: usize, seat: u8, kind: u8, bytes: &[u8]) -> Result<()> {
        ensure!(
            tick < CHUNK_TICKS && usize::from(seat) < SEATS,
            "invalid seat record"
        );
        ensure!(
            self.record_count < RECORD_LIMIT,
            "too many seat records in a chunk"
        );
        ensure!(
            self.record_payload + bytes.len() <= RECORD_BYTES,
            "seat records exceed their limit"
        );
        let len = u16::try_from(bytes.len()).context("seat record too long")?;
        self.records.put(&[tick as u8, seat, kind])?;
        self.records.put(&len.to_le_bytes())?;
        self.records.put(bytes)?;
        self.record_count += 1;
        self.record_payload += bytes.len();
        Ok(())
    }

    /// Close the chunk: the frame, ready to write. The next tick starts a
    /// new chunk.
    pub fn finish(&mut self, arena_hash: u64) -> Result<&[u8]> {
        let n = self.n;
        ensure!(n > 0, "empty film chunk");
        self.raw.len = 0;
        let mut seat_mask = 0u16;
        for s in 0..SEATS {
            if self.present[s * CHUNK_TICKS..s * CHUNK_TICKS + n]
                .iter()
                .any(|&p| p != 0)
            {
                seat_mask |= 1 << s;
            }
        }
        self.raw.put(&(n as u16).to_le_bytes())?;
        self.raw.put(&seat_mask.to_le_bytes())?;
        self.raw.put(&self.flags[..n])?;
        for t in 0..n {
            self.raw.put(&[self.digests[t] as u8])?;
        }
        for t in 0..n {
            self.raw.put(&[(self.digests[t] >> 8) as u8])?;
        }
        for s in 0..SEATS {
            if seat_mask & (1 << s) == 0 {
                continue;
            }
            let values = &self.values[s * CHUNK_TICKS * WORDS..(s * CHUNK_TICKS + n) * WORDS];
            let column = Column {
                values,
                n,
                p1: &self.state.p1[s],
                p2: &self.state.p2[s],
            };
            let present = &self.present[s * CHUNK_TICKS..s * CHUNK_TICKS + n];
            let mut mask = 0u16;
            if present.iter().any(|&p| p == 0) {
                mask |= 1;
            }
            for (i, field) in FIELDS.iter().enumerate() {
                if !column.holds(field) {
                    mask |= 1 << (i + 1);
                }
            }
            self.raw.put(&mask.to_le_bytes())?;
            if mask & 1 != 0 {
                self.raw.put(present)?;
            }
            for (i, field) in FIELDS.iter().enumerate() {
                if mask & (1 << (i + 1)) == 0 {
                    continue;
                }
                let choice = self.scratch.choose(&column, field);
                self.raw.put(&[choice.pred as u8])?;
                if choice.pred == Predictor::Grid {
                    self.raw.put(&choice.den.to_le_bytes())?;
                }
                self.raw.put(&self.scratch.best[..field.words * 4 * n])?;
                if choice.pred == Predictor::Grid {
                    let exc = &self.scratch.best_exc;
                    self.raw.put(&[exc.count as u8])?;
                    for k in 0..exc.count {
                        self.raw.put(&[exc.tick[k]])?;
                        self.raw.put(&exc.bits[k].to_le_bytes())?;
                    }
                }
            }
        }
        self.raw.put(&[self.record_count as u8])?;
        self.raw.put(&self.records.buf[..self.records.len])?;
        self.raw.put(&arena_hash.to_le_bytes())?;
        let size = self
            .zstd
            .compress_to_buffer(&self.raw.buf[..self.raw.len], &mut self.frame[..])
            .context("film chunk does not fit its frame")?;
        self.state.advance(&self.values, n);
        self.n = 0;
        self.records.len = 0;
        self.record_count = 0;
        self.record_payload = 0;
        Ok(&self.frame[..size])
    }
}

impl ChunkEncoder {
    /// The chunk as it stands, as [`ChunkEncoder::finish`] would make it,
    /// and leave the encoder as it was: the same ticks pending, the same
    /// predictor state. A cut film ends with a copy of the open chunk while
    /// the running film keeps filling it.
    pub fn finish_copy(&mut self, arena_hash: u64) -> Result<Vec<u8>> {
        let (n, state) = (self.n, self.state);
        let (len, count, payload) = (self.records.len, self.record_count, self.record_payload);
        let frame = self.finish(arena_hash)?.to_vec();
        self.n = n;
        self.state = state;
        self.records.len = len;
        self.record_count = count;
        self.record_payload = payload;
        Ok(frame)
    }
}

/// A seat record read back.
#[derive(Clone, Copy)]
struct Record {
    tick: u8,
    seat: u8,
    kind: u8,
    at: u32,
    len: u16,
}

pub struct ChunkDecoder {
    raw: Box<[u8]>,
    n: usize,
    flags: Box<[u8]>,
    digests: Box<[u16]>,
    present: Box<[u8]>,
    values: Box<[u32]>,
    state: State,
    res: Box<[u32]>,
    records: Box<[Record]>,
    record_count: usize,
    arena_hash: u64,
    zstd: zstd::bulk::Decompressor<'static>,
}

/// A cursor over the inflated chunk that refuses to read past it.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let (head, rest) = self
            .0
            .split_at_checked(len)
            .context("film chunk is cut short")?;
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

impl ChunkDecoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            raw: vec![0; RAW_CAP].into(),
            n: 0,
            flags: vec![0; CHUNK_TICKS].into(),
            digests: vec![0; CHUNK_TICKS].into(),
            present: vec![0; SEATS * CHUNK_TICKS].into(),
            values: vec![0; SEATS * CHUNK_TICKS * WORDS].into(),
            state: State::ZERO,
            res: vec![0; CHUNK_TICKS].into(),
            records: vec![
                Record {
                    tick: 0,
                    seat: 0,
                    kind: 0,
                    at: 0,
                    len: 0
                };
                RECORD_LIMIT
            ]
            .into(),
            record_count: 0,
            arena_hash: 0,
            zstd: zstd::bulk::Decompressor::new()?,
        })
    }

    /// Start again at the first tick of a film.
    pub fn reset(&mut self) {
        self.state = State::ZERO;
        self.n = 0;
    }

    /// The last two values of every word of every seat: the state the chunk
    /// after the last one loaded is decoded from. A sidecar keyframe stores
    /// it with its cursor, and a decoder resumed there starts from it.
    pub fn seat_rows(&self) -> ([[u32; WORDS]; SEATS], [[u32; WORDS]; SEATS]) {
        (self.state.p1, self.state.p2)
    }

    pub fn set_seat_rows(&mut self, p1: [[u32; WORDS]; SEATS], p2: [[u32; WORDS]; SEATS]) {
        self.state = State { p1, p2 };
        self.n = 0;
    }

    /// Inflate and decode one chunk whose first tick is `first_tick`. A
    /// chunk that is refused is refused with its tick range, and leaves the
    /// seat rows as they were; no tick of it can be read.
    pub fn load(&mut self, first_tick: u64, frame: &[u8]) -> Result<()> {
        self.n = 0;
        self.read(frame).with_context(|| {
            format!(
                "film chunk for ticks {first_tick} through {}",
                first_tick + CHUNK_TICKS as u64 - 1
            )
        })
    }

    fn read(&mut self, frame: &[u8]) -> Result<()> {
        // The frame's header descriptor says whether it carries a checksum,
        // which the inflater verifies.
        ensure!(
            frame.len() > 5 && frame[4] & 4 != 0,
            "film chunk has no checksum"
        );
        let len = self
            .zstd
            .decompress_to_buffer(frame, &mut self.raw[..])
            .context("film chunk does not inflate")?;
        let mut input = Cursor(&self.raw[..len]);
        let n = usize::from(input.u16()?);
        ensure!(
            (1..=CHUNK_TICKS).contains(&n),
            "film chunk tick count out of range"
        );
        let seat_mask = input.u16()?;
        self.flags[..n].copy_from_slice(input.take(n)?);
        let low = input.take(n)?;
        let high = input.take(n)?;
        for t in 0..n {
            self.digests[t] = u16::from(low[t]) | (u16::from(high[t]) << 8);
        }
        for s in 0..SEATS {
            let first = s * CHUNK_TICKS;
            if seat_mask & (1 << s) == 0 {
                self.present[first..first + n].fill(0);
                for t in 0..n {
                    let at = (first + t) * WORDS;
                    self.values[at..at + WORDS].copy_from_slice(&self.state.p1[s]);
                }
                continue;
            }
            let mask = input.u16()?;
            ensure!(
                mask >> (FIELDS.len() + 1) == 0,
                "film chunk names a field that does not exist"
            );
            if mask & 1 != 0 {
                let present = input.take(n)?;
                ensure!(
                    present.iter().all(|&p| p <= 1),
                    "film chunk presence is not a flag"
                );
                self.present[first..first + n].copy_from_slice(present);
            } else {
                self.present[first..first + n].fill(1);
            }
            for (i, field) in FIELDS.iter().enumerate() {
                if mask & (1 << (i + 1)) == 0 {
                    for t in 0..n {
                        for w in field.first..field.first + field.words {
                            self.values[(first + t) * WORDS + w] = self.state.p1[s][w];
                        }
                    }
                    continue;
                }
                let pred = Predictor::from_byte(input.u8()?)?;
                let mut den = 0;
                if pred == Predictor::Grid {
                    ensure!(
                        field.words == 1,
                        "film chunk puts a grid on a field of several words"
                    );
                    den = input.u32()?;
                    ensure!(
                        (1..=GRID_MAX).contains(&den),
                        "film chunk grid out of range"
                    );
                }
                let planes = input.take(field.words * 4 * n)?;
                // Values a grid cannot say, in tick order.
                let mut exceptions: &[u8] = &[];
                if pred == Predictor::Grid {
                    let count = usize::from(input.u8()?);
                    ensure!(
                        count <= n,
                        "film chunk lists more grid exceptions than ticks"
                    );
                    exceptions = input.take(count * 5)?;
                    let mut last = None;
                    for e in exceptions.chunks_exact(5) {
                        let tick = usize::from(e[0]);
                        ensure!(
                            tick < n && last.is_none_or(|l| l < tick),
                            "film chunk grid exceptions are out of order"
                        );
                        last = Some(tick);
                    }
                }
                for k in 0..field.words {
                    for t in 0..n {
                        self.res[t] = u32::from_le_bytes([
                            planes[(k * 4) * n + t],
                            planes[(k * 4 + 1) * n + t],
                            planes[(k * 4 + 2) * n + t],
                            planes[(k * 4 + 3) * n + t],
                        ]);
                    }
                    let w = field.first + k;
                    let (mut a, mut b) = (self.state.p1[s][w], self.state.p2[s][w]);
                    let mut next_exception = 0;
                    for t in 0..n {
                        let r = self.res[t];
                        let v = match pred {
                            Predictor::None => r,
                            Predictor::Previous => a.wrapping_add(unzig(r)),
                            Predictor::Linear => {
                                a.wrapping_mul(2).wrapping_sub(b).wrapping_add(unzig(r))
                            }
                            Predictor::Grid => {
                                let e = exceptions.get(next_exception * 5..next_exception * 5 + 5);
                                match e {
                                    Some(e) if usize::from(e[0]) == t => {
                                        next_exception += 1;
                                        u32::from_le_bytes([e[1], e[2], e[3], e[4]])
                                    }
                                    _ => from_step(
                                        step_of(a, den).wrapping_add(unzig(r) as i32),
                                        den,
                                    ),
                                }
                            }
                        };
                        self.values[(first + t) * WORDS + w] = v;
                        b = a;
                        a = v;
                    }
                }
            }
        }
        let count = usize::from(input.u8()?);
        let mut used = 0usize;
        for k in 0..count {
            let tick = input.u8()?;
            let seat = input.u8()?;
            let kind = input.u8()?;
            let size = input.u16()?;
            ensure!(
                usize::from(tick) < n && usize::from(seat) < SEATS,
                "film chunk seat record names a tick or seat that does not exist"
            );
            used += usize::from(size);
            ensure!(
                used <= RECORD_BYTES,
                "film chunk seat records exceed their limit"
            );
            let at = (len - input.0.len()) as u32;
            input.take(usize::from(size))?;
            self.records[k] = Record {
                tick,
                seat,
                kind,
                at,
                len: size,
            };
        }
        self.arena_hash = input.u64()?;
        ensure!(input.0.is_empty(), "film chunk has bytes after its end");
        self.state.advance(&self.values, n);
        self.record_count = count;
        self.n = n;
        Ok(())
    }

    pub fn ticks(&self) -> usize {
        self.n
    }

    pub fn flags(&self, tick: usize) -> u8 {
        self.flags[tick]
    }

    /// The low 16 bits of the tick's gameplay digest.
    pub fn digest(&self, tick: usize) -> u16 {
        self.digests[tick]
    }

    pub fn arena_hash(&self) -> u64 {
        self.arena_hash
    }

    /// The seat's action on the tick, written into `out`, or false if the
    /// seat was not in the game.
    pub fn action(&self, tick: usize, seat: usize, out: &mut [u8; ACTION_BYTES]) -> bool {
        if tick >= self.n || seat >= SEATS || self.present[seat * CHUNK_TICKS + tick] == 0 {
            return false;
        }
        let at = (seat * CHUNK_TICKS + tick) * WORDS;
        for (bytes, word) in out.chunks_exact_mut(4).zip(&self.values[at..at + WORDS]) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        true
    }

    pub fn seat_records(&self) -> impl Iterator<Item = SeatRecord<'_>> {
        self.records[..self.record_count]
            .iter()
            .map(|r| SeatRecord {
                tick: r.tick,
                seat: r.seat,
                kind: r.kind,
                bytes: &self.raw[r.at as usize..r.at as usize + usize::from(r.len)],
            })
    }
}

/// One field's row of `h2 film stat --fields`: how often it changes and
/// what each predictor stores it in. Bytes are zstd (level 3) of the
/// field's planes, a chunk of a seat at a time, over the seat-chunks in
/// which the field is not constant (a constant field is absent from the
/// chunk and costs nothing).
#[derive(Clone, Debug, Default)]
pub struct FieldRow {
    pub name: &'static str,
    pub words: usize,
    /// Seat-ticks on which the seat was in the game.
    pub seat_ticks: u64,
    /// ...and the field differs from the seat's last tick in the game (the
    /// first is against zero).
    pub changed: u64,
    /// Seat-chunks the seat was in the game on a tick of.
    pub seat_chunks: u64,
    /// ...and the field is not constant: the chunks that store it.
    pub stored_chunks: u64,
    /// Stored bytes of those chunks under none, previous and linear.
    pub stored: [u64; 3],
    /// Chunks where some grid of [`GRIDS`] holds with few enough
    /// exceptions, and the stored bytes (exceptions included) of the
    /// cheapest there. A field of several words has no grid.
    pub grid_chunks: u64,
    pub grid_stored: u64,
    /// For each of [`GRIDS`], the chunks it held in.
    pub grid_held: [u64; GRIDS.len()],
    /// The predictor the encoder keeps in a chunk, how many chunks each,
    /// and the bytes it stores.
    pub chosen: [u64; 4],
    pub chosen_stored: u64,
}

impl FieldRow {
    pub fn merge(&mut self, other: &FieldRow) {
        self.seat_ticks += other.seat_ticks;
        self.changed += other.changed;
        self.seat_chunks += other.seat_chunks;
        self.stored_chunks += other.stored_chunks;
        self.grid_chunks += other.grid_chunks;
        self.grid_stored += other.grid_stored;
        self.chosen_stored += other.chosen_stored;
        for i in 0..3 {
            self.stored[i] += other.stored[i];
        }
        for i in 0..GRIDS.len() {
            self.grid_held[i] += other.grid_held[i];
        }
        for i in 0..4 {
            self.chosen[i] += other.chosen[i];
        }
    }
}

/// The measurement slice 13 starts with, over the ticks of one film.
pub struct FieldStats {
    rows: Vec<FieldRow>,
    pending: Vec<[Option<[u8; ACTION_BYTES]>; SEATS]>,
    values: Vec<u32>,
    present: Vec<u8>,
    last: [[u32; WORDS]; SEATS],
    state: State,
    scratch: Scratch,
    planes: Vec<u8>,
    res: Vec<u32>,
    exc: Exceptions,
}

fn stored(bytes: &[u8]) -> Result<u64> {
    Ok(zstd::bulk::compress(bytes, 3)?.len() as u64)
}

impl FieldStats {
    pub fn new() -> Self {
        Self {
            rows: FIELDS
                .iter()
                .map(|f| FieldRow {
                    name: f.name,
                    words: f.words,
                    ..FieldRow::default()
                })
                .collect(),
            pending: Vec::with_capacity(CHUNK_TICKS),
            values: vec![0; SEATS * CHUNK_TICKS * WORDS],
            present: vec![0; SEATS * CHUNK_TICKS],
            last: [[0; WORDS]; SEATS],
            state: State::ZERO,
            scratch: Scratch::new(),
            planes: vec![0; 9 * 4 * CHUNK_TICKS],
            res: vec![0; CHUNK_TICKS],
            exc: Exceptions::new(),
        }
    }

    pub fn push_tick(&mut self, seats: &[Option<[u8; ACTION_BYTES]>; SEATS]) -> Result<()> {
        for (s, action) in seats.iter().enumerate() {
            let Some(action) = action else { continue };
            let mut words = [0u32; WORDS];
            words_of(action, &mut words);
            for (i, field) in FIELDS.iter().enumerate() {
                let row = &mut self.rows[i];
                row.seat_ticks += 1;
                let range = field.first..field.first + field.words;
                if words[range.clone()] != self.last[s][range] {
                    row.changed += 1;
                }
            }
            self.last[s] = words;
        }
        self.pending.push(*seats);
        if self.pending.len() == CHUNK_TICKS {
            self.chunk()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Vec<FieldRow>> {
        if !self.pending.is_empty() {
            self.chunk()?;
        }
        Ok(self.rows)
    }

    fn chunk(&mut self) -> Result<()> {
        let n = self.pending.len();
        for (t, seats) in self.pending.iter().enumerate() {
            for (s, action) in seats.iter().enumerate() {
                let at = (s * CHUNK_TICKS + t) * WORDS;
                self.present[s * CHUNK_TICKS + t] = u8::from(action.is_some());
                match action {
                    Some(action) => words_of(action, &mut self.values[at..at + WORDS]),
                    None if t == 0 => {
                        self.values[at..at + WORDS].copy_from_slice(&self.state.p1[s])
                    }
                    None => self.values.copy_within(at - WORDS..at, at),
                }
            }
        }
        for s in 0..SEATS {
            if self.present[s * CHUNK_TICKS..s * CHUNK_TICKS + n]
                .iter()
                .all(|&p| p == 0)
            {
                continue;
            }
            let values = &self.values[s * CHUNK_TICKS * WORDS..(s * CHUNK_TICKS + n) * WORDS];
            let column = Column {
                values,
                n,
                p1: &self.state.p1[s],
                p2: &self.state.p2[s],
            };
            for (i, field) in FIELDS.iter().enumerate() {
                let row = &mut self.rows[i];
                row.seat_chunks += 1;
                if column.holds(field) {
                    continue;
                }
                row.stored_chunks += 1;
                let size = field.words * 4 * n;
                for (k, pred) in [Predictor::None, Predictor::Previous, Predictor::Linear]
                    .into_iter()
                    .enumerate()
                {
                    column.planes(
                        field,
                        pred,
                        0,
                        &mut self.res,
                        &mut self.planes[..size],
                        &mut self.exc,
                    );
                    row.stored[k] += stored(&self.planes[..size])?;
                }
                if field.words == 1 {
                    let mut best: Option<(usize, usize)> = None;
                    for (g, den) in GRIDS.into_iter().enumerate() {
                        column.planes(
                            field,
                            Predictor::Grid,
                            den,
                            &mut self.res,
                            &mut self.planes[..size],
                            &mut self.exc,
                        );
                        if self.exc.count * 4 > n {
                            continue;
                        }
                        row.grid_held[g] += 1;
                        let c = cost(&self.planes[..size], n) + self.exc.count * 5;
                        if best.is_none_or(|(_, b)| c < b) {
                            best = Some((g, c));
                        }
                    }
                    if let Some((g, _)) = best {
                        column.planes(
                            field,
                            Predictor::Grid,
                            GRIDS[g],
                            &mut self.res,
                            &mut self.planes[..size],
                            &mut self.exc,
                        );
                        row.grid_chunks += 1;
                        row.grid_stored +=
                            stored(&self.planes[..size])? + self.exc.count as u64 * 5;
                    }
                }
                let choice = self.scratch.choose(&column, field);
                row.chosen[choice.pred as usize] += 1;
                let exceptions = if choice.pred == Predictor::Grid {
                    self.scratch.best_exc.count as u64 * 5
                } else {
                    0
                };
                row.chosen_stored += stored(&self.scratch.best[..size])? + exceptions;
            }
        }
        self.state.advance(&self.values, n);
        self.pending.clear();
        Ok(())
    }
}

impl Default for FieldStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
