use super::*;
use crate::{
    DATA, HASH, Source, Writer,
    blam::{
        Keyframe, KeyframeInfo, KeyframeSource, MatchHeader, SeatInput, Summary, Tick, TrackReader,
        TrackWriter,
    },
    manifest::Manifest,
    tape::{Batch, Call, Check, Playback},
    tests::header,
};
use std::time::Duration;

const LENGTH: u64 = 920;
const CAPTURE_INTERVAL: u64 = 150;
const STAMP: &str = "synthetic-build";
const SWITCHES: &str = "synthetic-switches";
const CAP: u64 = 1 << 20;

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("film-sidecar-{}", live_proto::sign::random_id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    path: PathBuf,
    id: String,
    frames: BTreeMap<u64, (Keyframe, u64)>,
}

fn call(index: u64) -> Call {
    Call {
        index,
        op: "Random".into(),
        args: vec![1],
        thread: 2,
        result: Ok((index * 17 % 179) as u32),
        outputs: vec![],
    }
}

fn digest(before: u64, pad: u8, call: &Call) -> u64 {
    before.rotate_left(7) ^ (u64::from(pad) << 8) ^ u64::from(call.result.unwrap())
}

fn fixture(
    root: &Path,
    id: u64,
    revision: u64,
    host: bool,
    embedded: bool,
    legacy_mark: bool,
) -> Fixture {
    let mut h = header(id);
    h.version = 3;
    h.meta.source = if host { Source::Host } else { Source::Local };
    h.meta.length_ticks = LENGTH;
    h.epoch_100ns = revision;
    let path = root.join(format!("{id}.h2film"));
    let mut out = Writer::new(File::create(&path).unwrap(), &h).unwrap();
    out.chunk(DATA, &Manifest::default()).unwrap();
    let mut track = TrackWriter::default();
    let mut batch = Batch::default();
    let mut state = 0;
    let mut frames = BTreeMap::new();
    for tick in 1..=LENGTH {
        let call = call(tick - 1);
        let pad = (tick / 190) as u8 + 1;
        state = digest(state, pad, &call);
        batch.append(&mut out, crate::RAND, call).unwrap();
        track
            .push(
                &mut out,
                Tick {
                    tick,
                    inputs: vec![SeatInput {
                        seat: 1,
                        pad: Some([pad; 18]),
                        action: None,
                    }],
                    post_hash: Some(state),
                    ..Tick::default()
                },
            )
            .unwrap();
        if tick % CAPTURE_INTERVAL == 0 {
            if !legacy_mark {
                batch.flush(&mut out).unwrap();
            }
            out.chunk(
                MARK,
                &Mark::D3dBarrier {
                    slices: tick * 3,
                    tick,
                    saved: true,
                },
            )
            .unwrap();
            let mark_offset = out.last_chunk_offset().unwrap();
            let frame = Keyframe {
                tick,
                slices: tick * 3,
                calls: tick,
                lobby_boundary: 0,
                snapshot: state.to_le_bytes().to_vec(),
                disk_delta: vec![tick as u8],
                ram_anchor: None,
            };
            if embedded && tick == CAPTURE_INTERVAL {
                track.keyframe(&mut out, &frame).unwrap();
            }
            frames.insert(tick, (frame, mark_offset));
        }
        if tick % 30 == 0 {
            batch.flush(&mut out).unwrap();
            out.chunk(
                HASH,
                &Check {
                    tick,
                    calls: tick,
                    sim: Some(state),
                },
            )
            .unwrap();
        }
    }
    batch.flush(&mut out).unwrap();
    track
        .finish(
            &mut out,
            &MatchHeader::default(),
            &Summary {
                duration_ticks: LENGTH,
                ..Summary::default()
            },
        )
        .unwrap();
    out.chunk(
        MARK,
        &Mark::End {
            slices: LENGTH * 3,
            tick: LENGTH,
            calls: LENGTH,
        },
    )
    .unwrap();
    out.finish().unwrap();
    Fixture {
        path,
        id: h.meta.id,
        frames,
    }
}

fn step(tape: &mut Playback, tick: &Tick, state: &mut u64) -> Option<bool> {
    let call = tape.call(tick.tick - 1, "Random", &[1], 2).unwrap();
    *state = digest(*state, tick.inputs[0].pad.unwrap()[0], &call);
    tick.verify(Some(*state)).unwrap();
    if tick.tick % 30 == 0 {
        let check = tape.checks.pop_front().unwrap();
        assert_eq!(
            check,
            Check {
                tick: tick.tick,
                calls: tick.tick,
                sim: Some(*state)
            }
        );
    }
    tape.d3d_barrier(tick.tick * 3, tick.tick).unwrap()
}

fn append(writer: &mut SidecarWriter, film: &Fixture, tick: u64, anchor: Option<u64>) {
    let (frame, offset) = &film.frames[&tick];
    writer.append(frame, anchor, *offset).unwrap();
}

fn publish(root: &Path, film: &Fixture, switches: &str, ticks: &[(u64, Option<u64>)]) -> PathBuf {
    let mut writer = SidecarWriter::new(root, &film.id, STAMP, switches, CAP).unwrap();
    for &(tick, anchor) in ticks {
        append(&mut writer, film, tick, anchor);
    }
    writer.finish(&film.path).unwrap();
    build_path(&root.join("cache"), &film.id, STAMP, switches).unwrap()
}

fn track(root: &Path, film: &Fixture, switches: &str) -> TrackReader<File> {
    let mut track = TrackReader::open(&film.path).unwrap();
    track.attach_sidecar(root, STAMP, switches).unwrap();
    track
}

#[test]
fn local_six_marks_and_one_sidecar_seek_match_straight_run_digests() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, false);
    let reader = Reader::open(&film.path).unwrap();
    assert!(!reader.entries.iter().any(|entry| entry.tag == KEYF));
    assert!(reader.ranges.as_ref().unwrap().keyframes.is_empty());
    assert_eq!(reader.header.meta.source, Source::Local);

    let mut tape = Playback::load(reader).unwrap();
    let mut track_before = track(&temp.0, &film, SWITCHES);
    assert!(track_before.seek(910).unwrap().is_none());
    let mut writer = SidecarWriter::new(&temp.0, &film.id, STAMP, SWITCHES, CAP).unwrap();
    let mut digest_at = Vec::new();
    let mut state = 0;
    let mut marks = 0;
    while let Some(tick) = track_before.step().unwrap() {
        if step(&mut tape, &tick, &mut state).is_some() {
            marks += 1;
            let point = tape.sidecar_capture_point().unwrap();
            assert_eq!(point.ordinal, marks);
            assert_eq!(point.tick, marks * CAPTURE_INTERVAL);
            if point.ordinal == 6 {
                let mut frame = film.frames[&tick.tick].0.clone();
                frame.snapshot = state.to_le_bytes().to_vec();
                writer
                    .append_at(&frame, None, SidecarCursor::new(point.offset))
                    .unwrap();
            }
        }
        digest_at.push(state);
    }
    assert!(tape.exhausted());
    assert_eq!(marks, 6);
    writer.finish(&film.path).unwrap();

    let mut track = track(&temp.0, &film, SWITCHES);
    assert_eq!(track.sidecar_ticks(), vec![900]);
    assert_eq!(
        track.keyframes(),
        vec![KeyframeInfo {
            tick: 900,
            source: KeyframeSource::Sidecar
        }]
    );
    let restored = track.seek(910).unwrap().unwrap();
    assert_eq!(restored.tick, 900);
    assert_eq!(track.keyframe_source(), Some(KeyframeSource::Sidecar));
    assert!(track.keyframe_offset().is_none());
    let mut tape = Playback::load(Reader::open(&film.path).unwrap()).unwrap();
    tape.resume_sidecar_at(track.sidecar_cursor().unwrap(), &restored)
        .unwrap();
    let mut state = u64::from_le_bytes(restored.snapshot.try_into().unwrap());
    let mut steps = 0;
    while let Some(tick) = track.step().unwrap() {
        assert_eq!(tick.tick, restored.tick + steps + 1);
        assert_eq!(step(&mut tape, &tick, &mut state), None);
        assert_eq!(state, digest_at[(tick.tick - 1) as usize]);
        steps += 1;
    }
    assert_eq!(steps, LENGTH - restored.tick);
    assert!(tape.exhausted());
}

#[test]
fn host_has_six_capture_points_without_a_keyframe_cache() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, true, false, false);
    let mut track = track(&temp.0, &film, SWITCHES);
    let reader = Reader::open(&film.path).unwrap();
    assert_eq!(reader.header.meta.source, Source::Host);
    assert!(reader.ranges.as_ref().unwrap().keyframes.is_empty());
    assert!(!reader.entries.iter().any(|entry| entry.tag == KEYF));
    let mut tape = Playback::load(reader).unwrap();
    let mut state = 0;
    let mut marks = 0;
    while let Some(tick) = track.step().unwrap() {
        if step(&mut tape, &tick, &mut state).is_some() {
            marks += 1;
        }
    }
    assert_eq!(marks, 6);
    assert!(track.seek(LENGTH).unwrap().is_none());
    assert!(!temp.0.join("cache").exists());
}

#[test]
fn builds_and_sealed_headers_select_independent_generations() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, false);
    let first = publish(&temp.0, &film, SWITCHES, &[(150, None)]);
    assert!(
        track(&temp.0, &film, "changed-switches")
            .seek(LENGTH)
            .unwrap()
            .is_none()
    );
    let second = publish(&temp.0, &film, "changed-switches", &[(300, None)]);
    assert_ne!(first, second);
    assert_eq!(
        track(&temp.0, &film, SWITCHES)
            .seek(LENGTH)
            .unwrap()
            .unwrap()
            .tick,
        150
    );
    assert_eq!(
        track(&temp.0, &film, "changed-switches")
            .seek(LENGTH)
            .unwrap()
            .unwrap()
            .tick,
        300
    );

    let replacement = fixture(&temp.0, 1, 1, false, false, false);
    assert!(
        track(&temp.0, &replacement, SWITCHES)
            .seek(LENGTH)
            .unwrap()
            .is_none()
    );
    assert!(!first.exists());
    assert_eq!(
        publish(&temp.0, &replacement, SWITCHES, &[(450, None)]),
        first
    );
    assert_eq!(
        track(&temp.0, &replacement, SWITCHES).sidecar_ticks(),
        vec![450]
    );
}

#[test]
fn embedded_ties_win_and_can_be_disabled_independently() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, true, false);
    publish(&temp.0, &film, SWITCHES, &[(150, None), (300, Some(150))]);
    let mut track = track(&temp.0, &film, SWITCHES);
    assert_eq!(
        track.keyframes(),
        vec![
            KeyframeInfo {
                tick: 150,
                source: KeyframeSource::Embedded
            },
            KeyframeInfo {
                tick: 300,
                source: KeyframeSource::Sidecar
            },
        ],
    );
    assert_eq!(track.seek(160).unwrap().unwrap().tick, 150);
    assert_eq!(track.keyframe_source(), Some(KeyframeSource::Embedded));
    assert!(track.keyframe_offset().is_some());
    let frame = track.seek(400).unwrap().unwrap();
    assert_eq!(frame.tick, 300);
    assert_eq!(
        frame.ram_anchor,
        Some((150, film.frames[&150].0.snapshot.clone()))
    );
    assert_eq!(track.keyframe_source(), Some(KeyframeSource::Sidecar));
    track.disable_embedded_keyframes();
    track.seek(160).unwrap().unwrap();
    assert_eq!(track.keyframe_source(), Some(KeyframeSource::Sidecar));
    assert_eq!(track.keyframes()[0].source, KeyframeSource::Sidecar);
}

fn damage_payload(path: &Path, tick: u64) {
    let reader = SidecarReader::read_index(path.into(), File::open(path).unwrap()).unwrap();
    let key = &reader.keys[reader.nearest(tick).unwrap()];
    let pos = key.offset + 8 + u64::from(key.size) - 1;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(pos)).unwrap();
    let mut c = [0];
    file.read_exact(&mut c).unwrap();
    file.seek(SeekFrom::Start(pos)).unwrap();
    file.write_all(&[c[0] ^ 1]).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn damaged_cache_falls_back_and_can_be_rebuilt() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, false);
    let path = publish(&temp.0, &film, SWITCHES, &[(150, None), (300, Some(150))]);
    damage_payload(&path, 150);
    let mut lazy = track(&temp.0, &film, SWITCHES);
    assert_eq!(lazy.sidecar_ticks(), vec![150, 300]);
    // Disk lookup needs only the selected frame. Seek also validates its anchor.
    assert_eq!(
        lazy.disk_delta_at(350).unwrap(),
        film.frames[&300].0.disk_delta
    );
    assert!(lazy.seek(350).unwrap().is_none());
    assert!(lazy.sidecar_ticks().is_empty());
    assert!(lazy.keyframe_source().is_none());
    assert_eq!(lazy.step().unwrap().unwrap().tick, 1);
    assert!(!path.exists());

    publish(&temp.0, &film, SWITCHES, &[(300, None)]);
    damage_payload(&path, 300);
    let mut prepare = track(&temp.0, &film, SWITCHES);
    prepare.verify_sidecar().unwrap();
    assert!(!path.exists());
    assert!(prepare.sidecar_ticks().is_empty());

    publish(&temp.0, &film, SWITCHES, &[(300, None)]);
    fs::write(&path, MAGIC).unwrap();
    assert!(track(&temp.0, &film, SWITCHES).seek(350).unwrap().is_none());
    assert!(!path.exists());
    publish(&temp.0, &film, SWITCHES, &[(300, None)]);
    assert_eq!(
        track(&temp.0, &film, SWITCHES)
            .seek(350)
            .unwrap()
            .unwrap()
            .tick,
        300
    );
}

#[test]
fn resume_extends_sparse_cache_and_old_reader_cannot_remove_new_generation() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, false);
    let path = publish(&temp.0, &film, SWITCHES, &[(300, None)]);
    let mut old = track(&temp.0, &film, SWITCHES);
    let mut reader = Reader::open(&film.path).unwrap();
    let mut writer = SidecarWriter::resume(&temp.0, &mut reader, STAMP, SWITCHES, CAP).unwrap();
    assert!(writer.contains(300));
    append(&mut writer, &film, 150, None);
    append(&mut writer, &film, 450, Some(300));
    writer.finish(&film.path).unwrap();
    old.invalidate_sidecar().unwrap();
    assert!(path.exists());
    assert_eq!(
        track(&temp.0, &film, SWITCHES).sidecar_ticks(),
        vec![150, 300, 450]
    );
}

#[test]
fn cache_cap_evicts_least_recently_watched_film_and_honors_other_build_watches() {
    let temp = Temp::new();
    let older = fixture(&temp.0, 1, 0, false, false, false);
    let newer = fixture(&temp.0, 2, 0, false, false, false);
    let a = publish(&temp.0, &older, SWITCHES, &[(150, None)]);
    let b = publish(&temp.0, &newer, SWITCHES, &[(150, None)]);
    let a_bytes = fs::metadata(&a).unwrap().len();
    let b_bytes = fs::metadata(&b).unwrap().len();
    for (path, age) in [(&a, 1), (&b, 2)] {
        let marker = path.parent().unwrap().join(WATCHED);
        File::options()
            .write(true)
            .open(marker)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(age))
            .unwrap();
    }
    assert!(
        track(&temp.0, &older, "another-build")
            .sidecar_ticks()
            .is_empty()
    );
    prune(&temp.0, a_bytes + b_bytes - 1).unwrap();
    assert!(a.exists());
    assert!(!b.exists());
    let mut writer = SidecarWriter::new(&temp.0, &newer.id, STAMP, SWITCHES, 1).unwrap();
    let (frame, mark) = &newer.frames[&150];
    assert!(writer.append(frame, None, *mark).is_err());
    drop(writer);
    assert!(a.exists());
    prune(&temp.0, 0).unwrap();
    assert!(!a.exists());
}

#[test]
fn deleting_a_film_cancels_staged_publication_and_unsupported_ids_are_ignored() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, false);
    let mut writer = SidecarWriter::new(&temp.0, &film.id, STAMP, SWITCHES, CAP).unwrap();
    append(&mut writer, &film, 150, None);
    remove_film(&temp.0, &film.id).unwrap();
    assert!(writer.finish(&film.path).is_err());
    remove_film(&temp.0, &film.id).unwrap();
    remove_film(&temp.0, "..").unwrap();
    assert!(film.path.exists());
    assert!(track(&temp.0, &film, SWITCHES).sidecar_ticks().is_empty());
}

#[test]
fn legacy_mark_before_call_flush_is_not_a_safe_sidecar_cursor() {
    let temp = Temp::new();
    let film = fixture(&temp.0, 1, 0, false, false, true);
    let mut track = track(&temp.0, &film, SWITCHES);
    let mut tape = Playback::load(Reader::open(&film.path).unwrap()).unwrap();
    let mut state = 0;
    while let Some(tick) = track.step().unwrap() {
        if step(&mut tape, &tick, &mut state).is_some() {
            let mark = tape.last_capture_point().unwrap();
            assert_eq!(mark.ordinal, 1);
            assert_eq!(mark.tick, 150);
            assert!(mark.saved);
            assert!(tape.sidecar_capture_point().is_none());
            break;
        }
    }
    let (frame, offset) = &film.frames[&150];
    let mut fresh = Playback::load(Reader::open(&film.path).unwrap()).unwrap();
    assert!(fresh.resume_sidecar(*offset, frame).is_err());
}
