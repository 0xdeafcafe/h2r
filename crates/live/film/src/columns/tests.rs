use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Counts the allocations of the thread that turned counting on.
struct Counting;

fn note() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
    }
}

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new) }
    }
}

#[global_allocator]
static COUNT: Counting = Counting;

fn allocations(run: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|n| n.set(0));
    COUNTING.with(|c| c.set(true));
    run();
    COUNTING.with(|c| c.set(false));
    ALLOCATIONS.with(Cell::get)
}

type Row = [Option<[u8; ACTION_BYTES]>; SEATS];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn put(action: &mut [u8; ACTION_BYTES], word: usize, bits: u32) {
    action[word * 4..word * 4 + 4].copy_from_slice(&bits.to_le_bytes());
}

/// A synthetic game of `ticks` ticks. Seat 0 plays: it turns steadily, its
/// move stick sits on a 1/127 grid with the odd stray value, its buttons
/// toggle, its assist block moves now and then and its tail takes noise
/// once in a while. Seat 1 is in the game and idle. Seat 2 joins at tick
/// 300 and leaves at 500. Seat 5 is in the game on every other tick.
fn game(ticks: usize) -> Vec<Row> {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut yaw = 0.0f32;
    let mut rows = Vec::new();
    let mut seat0 = [0u8; ACTION_BYTES];
    for t in 0..ticks {
        let mut row: Row = [None; SEATS];
        yaw += 0.0125;
        put(&mut seat0, 1, yaw.to_bits());
        put(
            &mut seat0,
            2,
            (0.5 + (t as f32 * 0.003).sin() * 0.25).to_bits(),
        );
        let step = (rng.next() % 255) as i32 - 127;
        let stick = if t % 97 == 0 {
            0.123_456_79
        } else {
            step as f32 / 127.0
        };
        put(&mut seat0, 3, stick.to_bits());
        if t % 40 == 0 {
            put(&mut seat0, 0, (rng.next() & 0xff) as u32);
        }
        if t % 23 == 0 {
            put(
                &mut seat0,
                13 + (rng.next() % 9) as usize,
                (rng.next() as f32).to_bits(),
            );
        }
        if t % 61 == 0 {
            put(&mut seat0, 22, rng.next() as u32);
        }
        row[0] = Some(seat0);
        row[1] = Some([0; ACTION_BYTES]);
        if (300..500).contains(&t) {
            let mut a = [0u8; ACTION_BYTES];
            put(&mut a, 1, (t as f32).to_bits());
            row[2] = Some(a);
        }
        if t % 2 == 0 {
            let mut a = [7u8; ACTION_BYTES];
            put(&mut a, 6, 0xdead_beef);
            row[5] = Some(a);
        }
        rows.push(row);
    }
    rows
}

fn encode(rows: &[Row]) -> Vec<Vec<u8>> {
    let mut encoder = ChunkEncoder::new().unwrap();
    let mut frames = Vec::new();
    for (i, chunk) in rows.chunks(CHUNK_TICKS).enumerate() {
        for (t, row) in chunk.iter().enumerate() {
            let tick = i * CHUNK_TICKS + t;
            encoder
                .push_tick(
                    tick as u8 ^ 0x5a,
                    0x1234_5678_0000_0000 | tick as u64 * 977,
                    row,
                )
                .unwrap();
        }
        encoder
            .seat_record(chunk.len() - 1, 3, 1, &[i as u8; 40])
            .unwrap();
        frames.push(encoder.finish(0xabcd_0000 + i as u64).unwrap().to_vec());
    }
    frames
}

fn check(rows: &[Row], frames: &[Vec<u8>]) {
    let mut decoder = ChunkDecoder::new().unwrap();
    for (i, (chunk, frame)) in rows.chunks(CHUNK_TICKS).zip(frames).enumerate() {
        decoder.load((i * CHUNK_TICKS + 1) as u64, frame).unwrap();
        assert_eq!(decoder.ticks(), chunk.len());
        assert_eq!(decoder.arena_hash(), 0xabcd_0000 + i as u64);
        let records: Vec<_> = decoder.seat_records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(
            (
                records[0].tick as usize,
                records[0].seat,
                records[0].kind,
                records[0].bytes
            ),
            (chunk.len() - 1, 3, 1, &[i as u8; 40][..])
        );
        for (t, row) in chunk.iter().enumerate() {
            let tick = i * CHUNK_TICKS + t;
            assert_eq!(decoder.flags(t), tick as u8 ^ 0x5a);
            assert_eq!(
                decoder.digest(t),
                ((0x1234_5678_0000_0000u64 | tick as u64 * 977) as u16)
            );
            for (s, action) in row.iter().enumerate() {
                let mut out = [0u8; ACTION_BYTES];
                let present = decoder.action(t, s, &mut out);
                assert_eq!(present, action.is_some(), "tick {tick} seat {s}");
                if let Some(action) = action {
                    assert_eq!(&out, action, "tick {tick} seat {s}");
                }
            }
        }
    }
}

#[test]
fn a_synthetic_game_round_trips_across_chunks_joins_and_leaves() {
    let rows = game(700);
    check(&rows, &encode(&rows));
}

#[test]
fn a_chunk_of_one_tick_and_an_empty_seat_set_round_trip() {
    let rows = vec![[None; SEATS]; 3];
    check(&rows, &encode(&rows));
    check(&rows[..1], &encode(&rows[..1]));
}

#[test]
fn every_bit_pattern_of_a_word_survives_every_predictor() {
    // NaNs, infinities, negative zero and the extremes, in a single field,
    // so the grid's exceptions and the integer wraps all run.
    let patterns = [
        0u32,
        1,
        0x8000_0000,
        0x7fc0_0000,
        0xffc0_0001,
        0x7f80_0000,
        0xff80_0000,
        u32::MAX,
        0x3f80_0000,
        0xbf80_0000,
    ];
    let mut rng = Rng(99);
    let rows: Vec<Row> = (0..300)
        .map(|t| {
            let mut a = [0u8; ACTION_BYTES];
            for w in 0..WORDS {
                let bits = if t % 3 == 0 {
                    patterns[(rng.next() % 10) as usize]
                } else {
                    rng.next() as u32
                };
                put(&mut a, w, bits);
            }
            let mut row: Row = [None; SEATS];
            row[15] = Some(a);
            row
        })
        .collect();
    check(&rows, &encode(&rows));
}

#[test]
fn decoding_resumes_from_stored_seat_rows() {
    let rows = game(600);
    let frames = encode(&rows);
    let mut straight = ChunkDecoder::new().unwrap();
    straight.load(1, &frames[0]).unwrap();
    let (p1, p2) = straight.seat_rows();
    let mut resumed = ChunkDecoder::new().unwrap();
    resumed.set_seat_rows(p1, p2);
    resumed.load(257, &frames[1]).unwrap();
    straight.load(257, &frames[1]).unwrap();
    for t in 0..CHUNK_TICKS {
        for s in 0..SEATS {
            let (mut a, mut b) = ([0; ACTION_BYTES], [0; ACTION_BYTES]);
            assert_eq!(resumed.action(t, s, &mut a), straight.action(t, s, &mut b));
            assert_eq!(a, b);
        }
    }
}

#[test]
fn the_columns_are_a_fraction_of_the_rows_and_of_the_rows_under_zstd() {
    let rows = game(CHUNK_TICKS);
    let frames = encode(&rows);
    let raw: Vec<u8> = rows.iter().flatten().flatten().flatten().copied().collect();
    let plain = zstd::bulk::compress(&raw, 3).unwrap().len();
    let frame = frames[0].len();
    // Two seats playing, one joining and one on every other tick.
    assert!(
        frame * 4 < raw.len(),
        "frame {frame} against {} raw bytes",
        raw.len()
    );
    assert!(
        frame < plain,
        "frame {frame} against {plain} for the rows under zstd"
    );
}

#[test]
fn a_steady_turn_costs_almost_nothing_under_the_linear_predictor() {
    let rows: Vec<Row> = (0..CHUNK_TICKS)
        .map(|t| {
            let mut a = [0u8; ACTION_BYTES];
            // The bit pattern moves by a constant step, as a steady turn's does.
            put(&mut a, 1, 0x3f00_0000 + 977 * t as u32);
            let mut row: Row = [None; SEATS];
            row[0] = Some(a);
            row
        })
        .collect();
    let frames = encode(&rows);
    check(&rows, &frames);
    assert!(frames[0].len() < 140, "{}", frames[0].len());
}

#[test]
fn a_refused_chunk_is_refused_with_its_ticks_and_leaves_the_seat_rows() {
    let rows = game(512);
    let frames = encode(&rows);
    let mut decoder = ChunkDecoder::new().unwrap();
    decoder.load(1, &frames[0]).unwrap();
    let rows_before = decoder.seat_rows();
    let mut damaged = frames[1].clone();
    let at = damaged.len() / 2;
    damaged[at] ^= 0x10;
    let error = format!("{:#}", decoder.load(257, &damaged).unwrap_err());
    assert!(error.contains("ticks 257 through 512"), "{error}");
    assert_eq!(decoder.ticks(), 0);
    assert!(decoder.seat_rows() == rows_before);
    decoder.load(257, &frames[1]).unwrap();
    assert!(
        decoder
            .load(513, &frames[1][..frames[1].len() - 1])
            .is_err()
    );
    assert!(decoder.load(513, &[]).is_err());
}

/// The inflated chunk of `rows`, and a frame of a corrupted copy of it
/// that zstd takes as sound (its checksum is the corrupted bytes').
fn corrupted(raw: &[u8]) -> Vec<u8> {
    let mut compressor = zstd::bulk::Compressor::new(3).unwrap();
    compressor
        .set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))
        .unwrap();
    compressor.compress(raw).unwrap()
}

#[test]
fn a_fuzzed_chunk_is_refused_and_never_read_out_of_bounds() {
    let rows = game(CHUNK_TICKS);
    let mut encoder = ChunkEncoder::new().unwrap();
    for row in &rows {
        encoder.push_tick(0, 0, row).unwrap();
    }
    encoder.seat_record(3, 1, 2, b"seat").unwrap();
    encoder.finish(7).unwrap();
    let raw = encoder.raw().to_vec();
    let mut decoder = ChunkDecoder::new().unwrap();
    decoder.load(1, &corrupted(&raw)).unwrap();
    // Every prefix is a cut-short chunk.
    for len in 0..raw.len() {
        assert!(
            decoder.load(1, &corrupted(&raw[..len])).is_err(),
            "prefix {len}"
        );
    }
    let mut extra = raw.clone();
    extra.push(0);
    assert!(decoder.load(1, &corrupted(&extra)).is_err());
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut refused = 0;
    for _ in 0..3000 {
        let mut copy = raw.clone();
        for _ in 0..=(rng.next() % 3) {
            let at = (rng.next() % copy.len() as u64) as usize;
            copy[at] = rng.next() as u8;
        }
        if decoder.load(1, &corrupted(&copy)).is_err() {
            refused += 1;
        }
    }
    assert!(refused > 0);
    // Named refusals.
    let mut bad = raw.clone();
    bad[0..2].copy_from_slice(&0u16.to_le_bytes());
    assert!(decoder.load(1, &corrupted(&bad)).is_err());
    bad[0..2].copy_from_slice(&257u16.to_le_bytes());
    assert!(decoder.load(1, &corrupted(&bad)).is_err());
    // A frame without a checksum is refused before it is inflated.
    let plain = zstd::bulk::compress(&raw, 3).unwrap();
    let error = format!("{:#}", decoder.load(1, &plain).unwrap_err());
    assert!(error.contains("checksum"), "{error}");
}

#[test]
fn the_encoder_and_decoder_allocate_nothing_after_they_start() {
    let rows = game(3 * CHUNK_TICKS);
    let mut encoder = ChunkEncoder::new().unwrap();
    // The first chunk is the start.
    for row in &rows[..CHUNK_TICKS] {
        encoder.push_tick(0, 0, row).unwrap();
    }
    encoder.finish(1).unwrap();
    let encoded = allocations(|| {
        for chunk in rows[CHUNK_TICKS..].chunks(CHUNK_TICKS) {
            for row in chunk {
                encoder.push_tick(0, 0, row).unwrap();
            }
            encoder.seat_record(0, 0, 0, &[1, 2, 3]).unwrap();
            encoder.finish(1).unwrap();
        }
    });
    assert_eq!(encoded, 0, "the encoder allocated after it started");

    let frames = encode(&rows);
    let mut decoder = ChunkDecoder::new().unwrap();
    decoder.load(1, &frames[0]).unwrap();
    let mut out = [0u8; ACTION_BYTES];
    let mut seen = 0;
    let decoded = allocations(|| {
        for (i, frame) in frames.iter().enumerate().skip(1) {
            decoder.load((i * CHUNK_TICKS + 1) as u64, frame).unwrap();
            for t in 0..decoder.ticks() {
                for s in 0..SEATS {
                    seen += usize::from(decoder.action(t, s, &mut out));
                }
            }
            seen += decoder.seat_records().count();
        }
    });
    assert!(seen > 0);
    assert_eq!(decoded, 0, "the decoder allocated after it started");
}

#[test]
fn field_stats_count_changes_and_price_each_predictor() {
    let rows = game(CHUNK_TICKS + 40);
    let mut stats = FieldStats::new();
    for row in &rows {
        stats.push_tick(row).unwrap();
    }
    let table = stats.finish().unwrap();
    assert_eq!(table.len(), FIELDS.len());
    // Seat 0 turns on every tick, the idle seat 1 never moves and seat 5,
    // in the game on every other tick, settles on its first.
    let yaw = &table[1];
    assert_eq!(yaw.name, "yaw");
    assert_eq!((yaw.seat_ticks, yaw.changed), (296 + 296 + 148, 296 + 1));
    // Seats 0 and 1 are in two chunks and seat 5 in two; the field is
    // stored for seat 0's two and for seat 5's first.
    assert_eq!((yaw.seat_chunks, yaw.stored_chunks), (6, 3));
    assert_eq!(yaw.chosen.iter().sum::<u64>(), yaw.stored_chunks);
    assert!(yaw.stored.iter().all(|&bytes| bytes > 0));
    // The move stick sits on a 1/127 grid, with a stray value now and then.
    let stick = &table[3];
    assert_eq!(stick.name, "throttle_x");
    let den = GRIDS.iter().position(|&d| d == 127).unwrap();
    assert!(stick.grid_held[den] >= 2, "{:?}", stick.grid_held);
    assert!(stick.grid_chunks >= 2);
    // A field of several words has no grid.
    assert_eq!(table[11].grid_chunks, 0);
}
