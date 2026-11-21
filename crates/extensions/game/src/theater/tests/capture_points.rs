use super::*;
use crate::snapshot_glue::{BootMachine, Part, Shelf, save_parts};
use snapshot::{BuildKey, Codec, Snapshot, SnapshotWriter, Tag};
use std::{cell::Cell, path::Path};

const CACHE_BYTES: u64 = 1 << 20;

#[derive(Default)]
struct SyntheticDevice {
    attempts: Cell<u64>,
    state: Cell<u64>,
    hashes: Cell<u64>,
    fail_on: Option<u64>,
}

struct DevicePart<'a>(&'a SyntheticDevice);

impl Part for DevicePart<'_> {
    fn name(&self) -> &'static str {
        "d3d"
    }

    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        assert!(in_keyframe());
        let attempt = self.0.attempts.get() + 1;
        self.0.attempts.set(attempt);
        let state = self.0.state.get() + 1;
        self.0.state.set(state);
        if self.0.fail_on == Some(attempt) {
            return Err("synthetic device failed after guest writeback".into());
        }
        out.section(
            Tag::new("d3d"),
            1,
            Codec::Stored,
            state.to_le_bytes().to_vec(),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    fn load(&mut self, snap: &Snapshot<'_>) -> Result<(), String> {
        let raw = snap
            .require(Tag::new("d3d"), 1..=1)
            .map_err(|e| e.to_string())?;
        let state = u64::from_le_bytes(raw.try_into().map_err(|_| "invalid synthetic device")?);
        self.0.state.set(state);
        Ok(())
    }
}

impl SyntheticDevice {
    fn machine(&self, root: &Path, build: &BuildKey) -> BootMachine<'_> {
        BootMachine {
            parts: vec![Box::new(DevicePart(self))],
            checks: Box::new(Vec::new),
            shelf: Shelf::new(root, build.clone()),
            hash: Box::new(|| {
                self.hashes.set(self.hashes.get() + 1);
                let state = self.state.get();
                lockstep::RamHash {
                    total: state,
                    chunks: vec![state],
                }
            }),
            presents: 0,
            rt: None,
        }
    }
}

struct Fixture {
    dir: PathBuf,
    path: PathBuf,
    opts: crate::RunOpts,
    build: BuildKey,
}

impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("film-capture-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("synthetic.h2film");
        let opts = options(&dir);
        let build = BuildKey {
            stamp: "fixture".into(),
            switches: "capture-points".into(),
        };
        Self {
            dir,
            path,
            opts,
            build,
        }
    }

    fn record(&self, host: bool) -> Arc<Session> {
        Session::open(
            Some(self.path.clone()),
            None,
            Facts {
                light: host,
                ..host_facts()
            },
            b"xbe",
            &self.opts,
        )
        .unwrap()
        .unwrap()
    }

    fn replay(&self, host: bool, target: u64) -> Arc<Session> {
        Session::open(
            None,
            Some(Replay {
                path: self.path.clone(),
                cache: None,
                render_every: 0,
                unpaced: false,
                speed: 1.,
                seek_tick: target,
                paused_at_target: target != 0,
                presentation: Presentation::default(),
                restart: self.dir.join("restart"),
                camera: Default::default(),
                window_closed: None,
            }),
            Facts {
                light: host,
                ..host_facts()
            },
            b"xbe",
            &self.opts,
        )
        .unwrap()
        .unwrap()
    }

    fn mount(&self, session: &Session, build: &BuildKey) {
        session
            .mount_sidecar(&self.dir, CACHE_BYTES, build, &self.dir)
            .unwrap();
    }

    fn record_cadence(&self, host: bool, device: &SyntheticDevice) {
        let record = self.record(host);
        self.mount(&record, &self.build);
        let guest = arena();
        let mut machine = device.machine(&self.dir, &self.build);
        assert!(!record.capture_point_due());
        for tick in 1..=901 {
            record.checkpoint(&guest);
            if tick == 900 {
                record.record(film::RAND, random(1));
            }
            assert_eq!(record.capture_point_due(), tick % 150 == 0);
            if record.capture_point_due() {
                record.try_capture_point(tick * 2, 0, &mut machine);
            }
            if tick == 900 {
                record.record(film::RAND, random(2));
            }
        }
        record.finish(1803, true).unwrap();
        assert!(record.lock().complete);
    }

    fn marks_without_embedded_keys(&self) -> Vec<(u64, u64, bool)> {
        let mut reader = Reader::open(&self.path).unwrap();
        let entries: Vec<_> = reader.machine_section().entries().cloned().collect();
        assert!(entries.iter().all(|entry| entry.tag != film::KEYF));
        entries
            .iter()
            .filter(|entry| entry.tag == film::MARK)
            .filter_map(|entry| match reader.read(entry).unwrap() {
                Mark::D3dBarrier {
                    slices,
                    tick,
                    saved,
                } => Some((slices, tick, saved)),
                _ => None,
            })
            .collect()
    }

    fn sidecar_ticks(&self, build: &BuildKey) -> Vec<u64> {
        let mut track = film::blam::TrackReader::open(&self.path).unwrap();
        track
            .attach_sidecar(&self.dir, &build.stamp, &build.switches)
            .unwrap();
        track
            .keyframes()
            .into_iter()
            .map(|key| {
                assert_eq!(key.source, film::blam::KeyframeSource::Sidecar);
                key.tick
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn random(value: u32) -> Call {
    Call {
        index: 0,
        op: "Random".into(),
        args: vec![value],
        thread: 1,
        result: Ok(value),
        outputs: vec![],
    }
}

fn play_random(replay: &Session, value: u32) {
    let call = replay.next("Random", &[value], 1).unwrap();
    assert_eq!(call.result, Ok(value));
}

#[test]
fn local_capture_points_write_one_keyframe_and_playback_can_rebuild_it() {
    let fixture = Fixture::new();
    let recorded = SyntheticDevice {
        fail_on: Some(2),
        ..Default::default()
    };
    fixture.record_cadence(false, &recorded);
    let marks = fixture.marks_without_embedded_keys();
    let expected: Vec<_> = (1..=6).map(|i| (i * 300, i * 150, i != 2)).collect();
    assert_eq!(marks, expected);
    assert_eq!(recorded.attempts.get(), 6);
    assert_eq!(recorded.hashes.get(), 1);
    assert_eq!(fixture.sidecar_ticks(&fixture.build), [900]);

    // Another viewer build has no cached machine state and starts at boot.
    let viewer = BuildKey {
        switches: "viewer".into(),
        ..fixture.build.clone()
    };
    assert!(fixture.sidecar_ticks(&viewer).is_empty());
    let replay = fixture.replay(false, 0);
    fixture.mount(&replay, &viewer);
    assert!(replay.take_keyframe().is_none());
    let playback = SyntheticDevice {
        fail_on: Some(2),
        ..Default::default()
    };
    let guest = arena();
    for tick in 1..=901 {
        replay.checkpoint(&guest);
        if tick == 900 {
            play_random(&replay, 1);
        }
        for slice in [tick * 2 - 1, tick * 2] {
            replay
                .replay_capture_point(slice, || 0, || playback.machine(&fixture.dir, &viewer))
                .unwrap();
            assert_eq!(playback.attempts.get(), slice / 300);
        }
        if tick == 900 {
            play_random(&replay, 2);
        }
    }
    replay.finish(1803, true).unwrap();
    assert!(replay.lock().complete);
    assert_eq!(playback.state.get(), recorded.state.get());
    assert_eq!(playback.hashes.get(), 1);
    assert_eq!(fixture.sidecar_ticks(&viewer), [900]);

    let seek = fixture.replay(false, 901);
    fixture.mount(&seek, &viewer);
    {
        let state = seek.lock();
        let Mode::Replay(_, replay) = &state.mode else {
            unreachable!()
        };
        let info = state.seek_info(replay).unwrap();
        assert_eq!(info.source, agent_proto::theater::SeekSource::Sidecar);
    }
    let frame = seek.take_keyframe().unwrap();
    assert_eq!((frame.tick, frame.calls), (900, 1));
    let restored = SyntheticDevice::default();
    let slice = seek
        .restore_keyframe(frame, &mut restored.machine(&fixture.dir, &viewer), None)
        .unwrap();
    assert_eq!(slice, 1800);
    assert_eq!(restored.state.get(), playback.state.get());
    seek.replay_capture_point(
        slice,
        || 0,
        || panic!("restored barrier must not run twice"),
    )
    .unwrap();
    play_random(&seek, 2);
    seek.checkpoint(&guest);
    assert!(seek.lock().paused);
    seek.finish(1803, true).unwrap();
    assert!(seek.lock().complete);
}

#[test]
fn host_capture_points_run_the_device_without_saving_machine_state() {
    let fixture = Fixture::new();
    let device = SyntheticDevice::default();
    fixture.record_cadence(true, &device);
    let expected: Vec<_> = (1..=6).map(|i| (i * 300, i * 150, true)).collect();
    assert_eq!(fixture.marks_without_embedded_keys(), expected);
    assert_eq!(device.attempts.get(), 6);
    assert_eq!(device.hashes.get(), 0);
    assert!(!fixture.dir.join("cache").exists());
    assert!(fixture.sidecar_ticks(&fixture.build).is_empty());
}

#[test]
fn seek_restores_machine_and_replays_only_the_remaining_ticks() {
    let fixture = Fixture::new();
    let record = fixture.record(false);
    let guest = arena();
    for _ in 0..30 {
        record.checkpoint(&guest);
    }

    // Write the legacy MARK + embedded KEYF pair to exercise both old paths.
    let device = SyntheticDevice::default();
    let mut machine = device.machine(&fixture.dir, &fixture.build);
    let snapshot = record.with_keyframe(|| {
        let header = snapshot::Header {
            build: fixture.build.clone(),
            name: "film-30".into(),
            tick: 30,
            presents: 0,
            notes: vec![],
        };
        save_parts(&mut machine.parts, header, &*machine.hash).unwrap()
    });
    {
        let mut state = record.lock();
        let Mode::Record(rec) = &mut state.mode else {
            unreachable!()
        };
        let out = rec.out.as_mut().unwrap();
        out.chunk(
            film::MARK,
            &Mark::D3dBarrier {
                slices: 35,
                tick: 30,
                saved: true,
            },
        )
        .unwrap();
        rec.track
            .keyframe(
                out,
                &film::blam::Keyframe {
                    tick: 30,
                    slices: 35,
                    calls: 0,
                    lobby_boundary: 0,
                    snapshot,
                    disk_delta: Vec::new(),
                    ram_anchor: None,
                },
            )
            .unwrap();
    }
    record.checkpoint(&guest);
    record.finish(40, true).unwrap();

    let linear = fixture.replay(false, 0);
    fixture.mount(&linear, &fixture.build);
    let linear_device = SyntheticDevice::default();
    for _ in 0..30 {
        linear.checkpoint(&guest);
    }
    linear
        .replay_capture_point(34, || 0, || panic!("barrier is at slice 35"))
        .unwrap();
    linear
        .replay_capture_point(
            35,
            || 0,
            || linear_device.machine(&fixture.dir, &fixture.build),
        )
        .unwrap();
    linear.checkpoint(&guest);
    linear.finish(40, true).unwrap();
    assert_eq!(linear_device.attempts.get(), 1);
    assert_eq!(linear_device.hashes.get(), 0);

    let seek = fixture.replay(false, 31);
    fixture.mount(&seek, &fixture.build);
    let keyframe = seek.take_keyframe().unwrap();
    assert_eq!(keyframe.tick, 30);
    let start = seek.restore_keyframe(keyframe, &mut machine, None).unwrap();
    assert_eq!(start, 35);
    assert_eq!(seek.lock().tick, 30);
    seek.checkpoint(&guest);
    assert!(seek.lock().paused);
    seek.finish(40, true).unwrap();
    assert!(seek.lock().complete);
}
