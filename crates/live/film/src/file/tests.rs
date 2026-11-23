use super::*;
use crate::{
    Writer,
    blam::{MapIdentity, SeatInput, Tick, TrackWriter},
    manifest::Manifest,
    tape::{Batch, Call},
    tests::header,
};
use std::{fs, io::Cursor};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("film-file-{}", live_proto::sign::random_id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn seed() -> Seed {
    Seed {
        header: header(5),
        descriptor: MatchHeader {
            map: MapIdentity {
                name: "example".into(),
                origin: "mod".into(),
                sha256: "a".repeat(64),
                ..Default::default()
            },
            variant_blob: vec![1, 2, 3],
            ..Default::default()
        },
    }
}

fn action(tick: u64, seat: u64) -> [u8; ACTION_BYTES] {
    let mut a = [0u8; ACTION_BYTES];
    a[4..8].copy_from_slice(&((tick as f32 + seat as f32) * 0.25).to_bits().to_le_bytes());
    a[0] = (tick / 50) as u8;
    a
}

fn seats(tick: u64) -> [Option<[u8; ACTION_BYTES]>; SEATS] {
    let mut row = [None; SEATS];
    row[0] = Some(action(tick, 0));
    if tick > 300 {
        row[3] = Some(action(tick, 3));
    }
    row
}

fn film(ticks: u64) -> Vec<u8> {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    w.machine(*b"DATA", &zstd::bulk::compress(b"disk", 3).unwrap())
        .unwrap();
    for tick in 1..=ticks {
        w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
        if tick == 301 {
            w.seat_record(3, RECORD_JOIN, &[3, 0]).unwrap();
        }
    }
    w.machine(*b"TAPE", &zstd::bulk::compress(b"calls", 3).unwrap())
        .unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn a_film_file_round_trips_its_seed_chunks_and_machine_part() {
    let bytes = film(600);
    let mut r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    assert_eq!(r.seed, seed());
    assert!(r.sealed && r.flags & FLAG_MACHINE != 0);
    assert_eq!(
        (r.length_ticks, r.chunk_count(), r.ticks_per_second),
        (600, 3, 30)
    );
    let tags: Vec<_> = r.machine.iter().map(|b| b.tag).collect();
    assert_eq!(tags, [*b"DATA", *b"TAPE"]);
    let block = r.machine[1];
    assert_eq!(
        zstd::bulk::decompress(&r.read_block(&block).unwrap(), 64).unwrap(),
        b"calls"
    );
    let mut decoder = ChunkDecoder::new().unwrap();
    let mut buf = Vec::new();
    for chunk in 0..3 {
        r.read_chunk(chunk, &mut decoder, &mut buf).unwrap();
        for t in 0..decoder.ticks() {
            let tick = (chunk * CHUNK_TICKS + t) as u64 + 1;
            assert_eq!(decoder.digest(t), (tick * 77) as u16);
            for (s, expected) in seats(tick).iter().enumerate() {
                let mut out = [0u8; ACTION_BYTES];
                assert_eq!(decoder.action(t, s, &mut out), expected.is_some());
                if let Some(e) = expected {
                    assert_eq!(&out, e);
                }
            }
        }
    }
    assert_eq!(decoder.seat_records().count(), 0);
    assert!(r.read_chunk(3, &mut decoder, &mut buf).is_err());
}

#[test]
fn a_file_still_being_written_is_walked_to_its_last_whole_block() {
    let bytes = film(600);
    let sealed = FilmReader::open(Cursor::new(&bytes)).unwrap();
    // An unsealed file: the header's sealed flag and trailer are zero.
    let mut open = bytes.clone();
    open[10] &= !(FLAG_SEALED as u8);
    open[32..36].fill(0);
    open[40..48].fill(0);
    let end = open.len() - (4 + 3 * 8);
    open.truncate(end);
    let r = FilmReader::open(Cursor::new(&open)).unwrap();
    assert!(!r.sealed);
    assert_eq!((r.chunk_count(), r.length_ticks), (3, 768));
    // A torn last block is dropped.
    let torn = &open[..open.len() - 5];
    let r = FilmReader::open(Cursor::new(torn)).unwrap();
    assert_eq!(r.chunk_count(), 2);
    assert_eq!(sealed.chunk_count(), 3);
}

#[test]
fn a_damaged_film_file_is_refused() {
    let bytes = film(600);
    for (at, what) in [
        (0, "magic"),
        (8, "version"),
        (13, "seat limit"),
        (48, "dictionary"),
    ] {
        let mut bad = bytes.clone();
        bad[at] ^= 0x55;
        assert!(FilmReader::open(Cursor::new(&bad)).is_err(), "{what}");
    }
    // The trailer's counts and offsets must agree with the blocks.
    let mut bad = bytes.clone();
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert!(FilmReader::open(Cursor::new(&bad)).is_err());
    let mut bad = bytes.clone();
    bad[33] ^= 0x80;
    assert!(FilmReader::open(Cursor::new(&bad)).is_err());
    // Cut at every length, and bytes made up: an error, never a panic.
    for len in (0..bytes.len()).step_by(7) {
        let _ = FilmReader::open(Cursor::new(&bytes[..len]));
    }
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..500 {
        let mut bad = bytes.clone();
        for _ in 0..3 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let at = (x % bad.len() as u64) as usize;
            bad[at] = (x >> 20) as u8;
        }
        let _ = FilmReader::open(Cursor::new(&bad));
    }
}

fn container(path: &Path, ticks: u64) {
    let mut h = header(5);
    h.version = 2;
    h.meta.length_ticks = ticks;
    let mut out = Writer::new(File::create(path).unwrap(), &h).unwrap();
    out.chunk(crate::DATA, &Manifest::default()).unwrap();
    let mut track = TrackWriter::default();
    let mut batch = Batch::default();
    for tick in 1..=ticks {
        let mut inputs = vec![SeatInput {
            seat: 0,
            pad: Some([1; 18]),
            action: Some(action(tick, 0)),
        }];
        if tick > 300 {
            inputs.push(SeatInput {
                seat: 3,
                pad: None,
                action: Some(action(tick, 3)),
            });
        }
        batch
            .append(
                &mut out,
                Call {
                    index: tick - 1,
                    op: "XInputGetState".into(),
                    thread: 1,
                    args: vec![9],
                    result: Ok(0),
                    outputs: vec![vec![tick as u8; 22]],
                },
            )
            .unwrap();
        track
            .push(
                &mut out,
                Tick {
                    tick,
                    inputs,
                    events: if tick == 301 {
                        vec![Event::Join {
                            player: blam::Seat {
                                seat: 3,
                                ..Default::default()
                            },
                        }]
                    } else {
                        vec![]
                    },
                    closing_events: vec![],
                    post_hash: (tick % 30 == 0).then_some(tick * 1_000_003),
                    forge: vec![],
                },
            )
            .unwrap();
        if tick == 30 {
            batch.flush(&mut out).unwrap();
            let key = blam::Keyframe {
                tick,
                slices: 60,
                calls: 30,
                lobby_boundary: 0,
                disk_delta: vec![],
                snapshot: vec![0xf1; 99],
                ram_anchor: None,
            };
            track.keyframe(&mut out, &key).unwrap();
        }
    }
    batch.flush(&mut out).unwrap();
    track
        .finish(&mut out, &seed().descriptor, &blam::Summary::default())
        .unwrap();
    out.chunk(
        crate::MARK,
        &crate::tape::Mark::End {
            slices: 9,
            tick: ticks,
            calls: ticks,
        },
    )
    .unwrap();
    out.finish().unwrap();
}

#[test]
fn a_container_converts_to_a_film_file_that_decodes_to_the_same_actions() {
    let dir = Temp::new();
    let path = dir.0.join("match.h2film");
    container(&path, 700);
    let converted = write_beside(&path).unwrap();
    assert_eq!((converted.ticks, converted.chunks), (700, 3));
    // DATA, the seven tape batches and the final mark; no keyframe and no header.
    assert_eq!(converted.machine_blocks, 1 + 7 + 1);
    let r = FilmReader::open(File::open(beside(&path)).unwrap()).unwrap();
    assert!(
        r.machine
            .iter()
            .all(|b| b.tag != crate::KEYF && b.tag != crate::HEAD)
    );
    assert_eq!(verify(&path, &beside(&path)).unwrap(), 700);
    // Seat 3's join is a seat record on tick 301, in the chunk that holds it.
    let mut r = FilmReader::open(File::open(beside(&path)).unwrap()).unwrap();
    let (mut decoder, mut buf) = (ChunkDecoder::new().unwrap(), Vec::new());
    r.read_chunk(0, &mut decoder, &mut buf).unwrap();
    r.read_chunk(1, &mut decoder, &mut buf).unwrap();
    let records: Vec<_> = decoder
        .seat_records()
        .map(|r| (r.tick, r.seat, r.kind))
        .collect();
    assert_eq!(records, [(44, 3, RECORD_JOIN)]);
    // A film that differs from its container is refused.
    let mut damaged = fs::read(beside(&path)).unwrap();
    let at = damaged.len() / 2;
    damaged[at] ^= 0xff;
    let other = dir.0.join("damaged.h2f");
    fs::write(&other, damaged).unwrap();
    assert!(verify(&path, &other).is_err());
}

fn decode_all(bytes: &[u8]) -> (FilmReader<Cursor<&[u8]>>, Vec<(u16, Vec<Option<[u8; ACTION_BYTES]>>)>) {
    let mut r = FilmReader::open(Cursor::new(bytes)).unwrap();
    let mut decoder = ChunkDecoder::new().unwrap();
    let mut buf = Vec::new();
    let mut rows = Vec::new();
    for chunk in 0..r.chunk_count() {
        r.read_chunk(chunk, &mut decoder, &mut buf).unwrap();
        for t in 0..decoder.ticks() {
            let mut seats = Vec::new();
            for s in 0..SEATS {
                let mut out = [0u8; ACTION_BYTES];
                seats.push(decoder.action(t, s, &mut out).then_some(out));
            }
            rows.push((decoder.digest(t), seats));
        }
    }
    (r, rows)
}

#[test]
fn the_container_writer_mirrors_its_machine_chunks_into_the_film_file() {
    let film = Arc::new(Mutex::new(
        FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap(),
    ));
    let mut w = Writer::new(Cursor::new(Vec::new()), &header(5)).unwrap();
    w.set_mirror(Some(machine_mirror(Arc::clone(&film))));
    w.chunk_bytes(*b"DATA", b"disk").unwrap();
    // The container keeps what the film file does not carry.
    w.chunk_bytes(crate::KEYF, &[1, 2, 3, 4]).unwrap();
    w.chunk_bytes(*b"TAPE", b"calls").unwrap();
    w.set_mirror(None);
    w.chunk_bytes(*b"HASH", b"after").unwrap();
    let container = w.finish().unwrap().into_inner();
    let film = Arc::try_unwrap(film).ok().unwrap().into_inner().unwrap();
    let bytes = film.finish().unwrap().into_inner();
    let mut r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    let tags: Vec<_> = r.machine.iter().map(|b| b.tag).collect();
    assert_eq!(tags, [*b"DATA", *b"TAPE"]);
    let mut raw = Vec::new();
    for block in r.machine.clone() {
        raw.push(zstd::bulk::decompress(&r.read_block(&block).unwrap(), 64).unwrap());
    }
    assert_eq!(raw, [b"disk".to_vec(), b"calls".to_vec()]);
    // The stored frame is the container's own.
    let reader = Reader::new(Cursor::new(&container)).unwrap();
    let data = reader.entries.iter().find(|e| e.tag == *b"DATA").unwrap();
    let stored = &container[data.offset as usize + 8..][..data.size as usize];
    let block = r.machine[0];
    assert_eq!(r.read_block(&block).unwrap(), stored);
}

#[test]
fn a_cut_film_is_whole_while_the_running_film_goes_on() {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    w.machine(*b"DATA", &zstd::bulk::compress(b"disk", 3).unwrap())
        .unwrap();
    let push = |w: &mut FilmWriter<Cursor<Vec<u8>>>, from: u64, to: u64| {
        for tick in from..=to {
            w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
            if tick == 301 {
                w.seat_record(3, RECORD_JOIN, &[3, 0]).unwrap();
            }
        }
    };
    push(&mut w, 1, 600);
    let mut cut_seed = seed();
    cut_seed.header = header(6);
    let mut cut = w.fork(Cursor::new(Vec::new()), &cut_seed).unwrap();
    assert!(cut.push_tick(0, 0, &seats(1)).is_err());
    cut.machine(*b"MARK", &zstd::bulk::compress(b"end", 3).unwrap())
        .unwrap();
    let cut = cut.finish().unwrap().into_inner();
    // Cutting on a full chunk's last tick, a seat record still lands on it.
    push(&mut w, 601, 768);
    w.seat_record(1, RECORD_LEAVE, &[1]).unwrap();
    let full_cut = w.fork(Cursor::new(Vec::new()), &cut_seed).unwrap().finish().unwrap().into_inner();
    push(&mut w, 769, 800);
    let whole = w.finish().unwrap().into_inner();

    let (r, rows) = decode_all(&cut);
    assert_eq!(r.seed, cut_seed);
    assert!(r.sealed);
    assert_eq!((r.length_ticks, r.chunk_count(), rows.len()), (600, 3, 600));
    let tags: Vec<_> = r.machine.iter().map(|b| b.tag).collect();
    assert_eq!(tags, [*b"DATA", *b"MARK"]);
    for (i, (digest, row)) in rows.iter().enumerate() {
        let tick = i as u64 + 1;
        assert_eq!(*digest, (tick * 77) as u16);
        assert_eq!(row.as_slice(), seats(tick).as_slice());
    }

    let (r, rows) = decode_all(&full_cut);
    assert_eq!((r.length_ticks, r.chunk_count(), rows.len()), (768, 3, 768));

    let (r, rows) = decode_all(&whole);
    assert_eq!(r.seed, seed());
    assert_eq!((r.length_ticks, r.chunk_count(), rows.len()), (800, 4, 800));
    for (i, (digest, row)) in rows.iter().enumerate() {
        let tick = i as u64 + 1;
        assert_eq!(*digest, (tick * 77) as u16);
        assert_eq!(row.as_slice(), seats(tick).as_slice());
    }
}

#[test]
fn a_seat_record_follows_the_last_tick_of_a_full_chunk() {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    for tick in 1..=CHUNK_TICKS as u64 {
        w.push_tick(0, 0, &seats(tick)).unwrap();
    }
    w.seat_record(2, RECORD_LEAVE, &[2]).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let mut r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    let mut decoder = ChunkDecoder::new().unwrap();
    r.read_chunk(0, &mut decoder, &mut Vec::new()).unwrap();
    assert_eq!(decoder.seat_records().count(), 1);
}
