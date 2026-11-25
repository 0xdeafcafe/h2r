use super::*;
use crate::{
    Reader, Writer,
    blam::{self, Event, MapIdentity, SeatInput, Tick, TrackReader, TrackWriter},
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
                    firefight: vec![],
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

/// The film file of a container, made from its own ticks.
fn write_film(container: &Path) -> PathBuf {
    let mut track = TrackReader::open(container).unwrap();
    let seed = Seed {
        header: Reader::open(container).unwrap().header.clone(),
        descriptor: track.descriptor.clone(),
    };
    let film = beside(container);
    let mut w = FilmWriter::new(File::create(&film).unwrap(), &seed, 30).unwrap();
    while let Some(tick) = track.step().unwrap() {
        let flags = if tick.post_hash.is_some() { TICK_DIGEST } else { 0 };
        let mut seats = [None; SEATS];
        for input in &tick.inputs {
            seats[usize::from(input.seat)] = input.action;
        }
        w.push_tick(flags, tick.post_hash.unwrap_or(0), &seats).unwrap();
    }
    w.finish().unwrap();
    film
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

fn steps(track: &mut TrackReader<File>) -> Vec<Tick> {
    std::iter::from_fn(|| track.step().unwrap()).collect()
}

#[test]
fn a_track_reader_takes_its_actions_from_the_film_file() {
    let dir = Temp::new();
    let path = dir.0.join("match.h2film");
    container(&path, 700);
    write_film(&path);
    let plain = steps(&mut TrackReader::open(&path).unwrap());
    assert_eq!(plain.len(), 700);

    let mut merged = TrackReader::open(&path).unwrap();
    merged
        .attach_film(FilmActions::open(&beside(&path)).unwrap())
        .unwrap();
    assert_eq!(steps(&mut merged), plain);

    // Out of order: forward across chunks, back to an earlier one, and on.
    let mut merged = TrackReader::open(&path).unwrap();
    merged
        .attach_film(FilmActions::open(&beside(&path)).unwrap())
        .unwrap();
    let mut seek = TrackReader::open(&path).unwrap();
    for tick in [450u64, 100, 599, 255, 0, 300] {
        merged.resume_after_tick(tick).unwrap();
        seek.resume_after_tick(tick).unwrap();
        let (a, b) = (merged.step().unwrap(), seek.step().unwrap());
        assert_eq!(a, b, "after tick {tick}");
        assert_eq!(a.map(|t| t.tick), Some(tick + 1));
    }
}

#[test]
fn a_film_file_that_disagrees_with_its_stats_stream_is_refused() {
    let dir = Temp::new();
    let path = dir.0.join("match.h2film");
    container(&path, 100);
    // The right actions with a wrong digest on tick 30.
    let mut w = FilmWriter::new(File::create(beside(&path)).unwrap(), &seed(), 30).unwrap();
    for tick in 1..=100u64 {
        let digest = if tick == 30 { 1 } else { tick * 1_000_003 };
        let flags = if tick % 30 == 0 { TICK_DIGEST } else { 0 };
        w.push_tick(flags, digest, &seats(tick)).unwrap();
    }
    w.finish().unwrap();
    let mut track = TrackReader::open(&path).unwrap();
    track
        .attach_film(FilmActions::open(&beside(&path)).unwrap())
        .unwrap();
    // A block is checked as it is read, so the error comes at or before
    // the block that holds tick 30.
    assert!((0..100).any(|_| track.step().is_err()));

    // A film file of another film, or a shorter one, does not attach.
    let mut other = seed();
    other.header = header(9);
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &other, 30).unwrap();
    w.push_tick(0, 0, &seats(1)).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let mut track = TrackReader::open(&path).unwrap();
    assert!(
        track
            .attach_film(FilmActions::new(Box::new(Cursor::new(bytes))).unwrap())
            .is_err()
    );
}

#[test]
fn a_checkpoint_keeps_the_open_chunk_for_a_reader_of_an_unsealed_file() {
    let dir = Temp::new();
    let path = dir.0.join("match.h2f");
    let mut w = FilmWriter::new(File::create(&path).unwrap(), &seed(), 30).unwrap();
    for tick in 1..=300u64 {
        w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
    }
    w.checkpoint().unwrap();
    let mut r = FilmReader::open(File::open(&path).unwrap()).unwrap();
    assert!(!r.sealed);
    assert_eq!((r.chunk_count(), r.length_ticks), (2, 300));
    let (mut decoder, mut buf) = (ChunkDecoder::new().unwrap(), Vec::new());
    r.read_chunk(0, &mut decoder, &mut buf).unwrap();
    r.read_chunk(1, &mut decoder, &mut buf).unwrap();
    assert_eq!(decoder.ticks(), 44);
    assert_eq!(decoder.digest(43), (300u64 * 77) as u16);
    // The file goes on; the sealed file's chunks are the whole ones.
    for tick in 301..=600u64 {
        w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
    }
    w.checkpoint().unwrap();
    w.finish().unwrap();
    let r = FilmReader::open(File::open(&path).unwrap()).unwrap();
    assert!(r.sealed);
    assert_eq!((r.chunk_count(), r.length_ticks), (3, 600));
}

#[test]
fn a_killed_recordings_film_file_is_sealed_at_a_tick_its_checkpoint_covers() {
    let dir = Temp::new();
    let container = dir.0.join("tape.h2film");
    let mut w = FilmWriter::new(File::create(beside(&container)).unwrap(), &seed(), 30).unwrap();
    w.machine(*b"DATA", &zstd::bulk::compress(b"disk", 3).unwrap())
        .unwrap();
    for tick in 1..=700u64 {
        w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
        if tick == 301 {
            w.seat_record(3, RECORD_JOIN, &[3, 0]).unwrap();
        }
        if tick == 650 {
            w.checkpoint().unwrap();
        }
    }
    // The recorder died here: no trailer, ticks 651 to 700 never reached the file.
    drop(w);
    assert!(recover_beside(&container, 660).is_err());
    assert!(!beside(&container).with_extension("h2f.recover").exists());
    recover_beside(&container, 640).unwrap();
    let bytes = fs::read(beside(&container)).unwrap();
    let (mut r, rows) = decode_all(&bytes);
    assert!(r.sealed);
    assert_eq!((r.length_ticks, rows.len()), (640, 640));
    assert_eq!(r.machine.len(), 1);
    for (i, (digest, row)) in rows.iter().enumerate() {
        let tick = i as u64 + 1;
        assert_eq!(*digest, (tick * 77) as u16);
        assert_eq!(row.as_slice(), seats(tick).as_slice());
    }
    let (mut decoder, mut buf) = (ChunkDecoder::new().unwrap(), Vec::new());
    r.read_chunk(0, &mut decoder, &mut buf).unwrap();
    r.read_chunk(1, &mut decoder, &mut buf).unwrap();
    let records: Vec<_> = decoder.seat_records().map(|r| (r.tick, r.seat, r.kind)).collect();
    assert_eq!(records, [(44, 3, RECORD_JOIN)]);
}

fn started(first_update: u64) -> crate::start::GameStart {
    let mut start = crate::start::tests::sample();
    start.game.first_update = first_update;
    start.start_hash = first_update * 3;
    start
}

fn film_of_two_games() -> Vec<u8> {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    for tick in 1..=400u64 {
        if tick == 1 || tick == 91 {
            w.game_start(&started(tick)).unwrap();
        }
        w.push_tick(TICK_DIGEST, tick * 77, &seats(tick)).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn a_film_holds_a_game_start_for_each_game_at_its_first_update() {
    let bytes = film_of_two_games();
    let mut r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    assert!(r.flags & FLAG_GAME_START != 0);
    assert_eq!((r.game_starts.len(), r.length_ticks), (2, 400));
    let first = r.game_start_at(&r.game_starts[0].clone()).unwrap();
    assert_eq!((first.game.first_update, first.start_hash), (1, 3));
    // The film is the game played last.
    let last = r.game_start().unwrap().unwrap();
    assert_eq!(last, started(91));
    // A film from before game starts has none.
    let old = film(10);
    let mut r = FilmReader::open(Cursor::new(&old)).unwrap();
    assert!(r.game_start().unwrap().is_none() && r.flags & FLAG_GAME_START == 0);
}

#[test]
fn a_game_start_must_name_the_tick_the_writer_takes_next() {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    assert!(w.game_start(&started(2)).is_err());
    w.game_start(&started(1)).unwrap();
    w.push_tick(TICK_DIGEST, 1, &seats(1)).unwrap();
    assert!(w.game_start(&started(1)).is_err());
    let mut bad = started(2);
    bad.options.pop();
    assert!(w.game_start(&bad).is_err());
    w.game_start(&started(2)).unwrap();
}

#[test]
fn an_unsealed_film_and_a_fork_keep_their_game_starts() {
    let mut w = FilmWriter::new(Cursor::new(Vec::new()), &seed(), 30).unwrap();
    for tick in 1..=300u64 {
        if tick == 1 || tick == 91 {
            w.game_start(&started(tick)).unwrap();
        }
        w.push_tick(TICK_DIGEST, tick, &seats(tick)).unwrap();
    }
    let mut cut_seed = seed();
    cut_seed.header.meta.id = "the-second-game".into();
    let cut = w.fork(Cursor::new(Vec::new()), &cut_seed).unwrap();
    let bytes = cut.finish().unwrap().into_inner();
    let mut r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    assert_eq!(r.game_starts.len(), 2);
    assert_eq!(r.game_start().unwrap().unwrap().game.first_update, 91);
    // The recording went on untouched; read it as a file still being written.
    let live = w.out.get_mut().clone();
    let mut r = FilmReader::open(Cursor::new(&live)).unwrap();
    assert!(!r.sealed);
    assert_eq!(r.game_starts.len(), 2);
    assert_eq!(r.game_start().unwrap().unwrap(), started(91));
}

#[test]
fn a_damaged_game_start_block_is_refused_when_read_not_when_listed() {
    let bytes = film_of_two_games();
    let r = FilmReader::open(Cursor::new(&bytes)).unwrap();
    let at = r.game_starts[1].offset as usize + 2;
    let mut bad = bytes.clone();
    bad[at] ^= 0xff;
    let mut r = FilmReader::open(Cursor::new(&bad)).unwrap();
    assert!(r.game_start().is_err());
    // Every cut of the block's bytes: an error, never a panic.
    let block = r.game_starts[1];
    for len in 0..block.len as usize {
        assert!(crate::start::GameStart::decode(&bytes[block.offset as usize..][..len]).is_err());
    }
}

#[test]
fn a_tick_row_gives_the_actions_the_digest_and_the_chunks_closing_digest() {
    let bytes = film_of_two_games();
    let dir = std::env::temp_dir().join(format!("film-rows-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("two.h2f");
    fs::write(&path, &bytes).unwrap();
    let mut actions = FilmActions::open(&path).unwrap();
    assert_eq!(actions.game_start().unwrap(), Some(started(91)));
    assert_eq!(actions.length_ticks(), 400);
    let row = actions.row(91).unwrap();
    assert_eq!((row.digest, row.closing), (Some((91u64 * 77) as u16), None));
    let mut first = Vec::new();
    for seat in 0..SEATS {
        if let Some(action) = seats(91)[seat] {
            first.push((seat as u8, action));
        }
    }
    let got: Vec<_> = row.inputs.iter().map(|i| (i.seat, i.action.unwrap())).collect();
    assert_eq!(got, first);
    // The 256th tick closes the first chunk with its whole digest, and the
    // film's last tick closes the second.
    assert_eq!(actions.row(256).unwrap().closing, Some(256 * 77));
    assert_eq!(actions.row(400).unwrap().closing, Some(400 * 77));
    assert!(actions.row(401).is_err() && actions.row(0).is_err());
    fs::remove_dir_all(&dir).unwrap();
}
