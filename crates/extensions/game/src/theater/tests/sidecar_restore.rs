use super::*;
use crate::snapshot_glue::{BootMachine, Shelf, save_parts};

#[test]
fn cold_seek_pairs_the_disk_with_ram_and_retries_a_bad_ram_hash_from_boot() {
    let dir = std::env::temp_dir().join(format!("film-seek-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("synthetic.h2film");
    let opts = options(&dir);
    let record = Session::open(Some(path.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let build = snapshot::BuildKey {
        stamp: "fixture".into(),
        switches: "test".into(),
    };
    let guest = arena();
    record.checkpoint(&guest);
    let (id, mark_offset) = {
        let mut state = record.lock();
        let id = state.header.meta.id.clone();
        let Mode::Record(rec) = &mut state.mode else {
            unreachable!()
        };
        let out = rec.out.as_mut().unwrap();
        out.chunk(
            film::MARK,
            &Mark::D3dBarrier {
                tick: 1,
                slices: 2,
                saved: true,
            },
        )
        .unwrap();
        (id, out.last_chunk_offset().unwrap())
    };
    record.finish(3, true).unwrap();

    // A valid snapshot envelope whose saved RAM hash disagrees with this
    // synthetic machine after restore. The sidecar's byte checks all pass.
    let header = snapshot::Header {
        build: build.clone(),
        name: "film-1".into(),
        tick: 1,
        presents: 0,
        notes: vec![],
    };
    let bytes = save_parts(&mut [], header, &|| lockstep::RamHash {
        total: 1,
        chunks: vec![1],
    })
    .unwrap();
    let edited = dir.join("edited");
    std::fs::create_dir_all(edited.join("disk")).unwrap();
    std::fs::write(edited.join("disk/save"), b"synthetic checkpoint").unwrap();
    let manifest = film::manifest::Manifest::default();
    let delta = manifest.keyframe_delta(&edited).unwrap();
    let restored = dir.join("restored");
    manifest.restore_keyframe(&restored, None, &delta).unwrap();
    assert_eq!(
        std::fs::read(restored.join("disk/save")).unwrap(),
        b"synthetic checkpoint"
    );
    let frame = film::blam::Keyframe {
        tick: 1,
        slices: 2,
        calls: 0,
        lobby_boundary: 0,
        snapshot: bytes,
        ram_anchor: None,
        disk_delta: delta.clone(),
    };
    let cap = 1 << 20;
    let mut writer =
        film::sidecar::SidecarWriter::new(&dir, &id, &build.stamp, &build.switches, cap).unwrap();
    writer.append(&frame, None, mark_offset).unwrap();
    writer.finish(&path).unwrap();
    let open = |prepared_disk: &[u8]| {
        Session::open(
            None,
            Some(Replay {
                path: path.clone(),
                cache: Some(ReplayCache {
                    films_dir: dir.clone(),
                    cap_bytes: cap,
                    disk_digest: film::xbe_digest(prepared_disk),
                }),
                unpaced: false,
                render_every: 0,
                speed: 1.0,
                seek_tick: 1,
                paused_at_target: false,
                presentation: Presentation::default(),
                restart: dir.join("restart"),
                camera: Default::default(),
                window_closed: None,
            }),
            host_facts(),
            b"xbe",
            &opts,
        )
        .unwrap()
        .unwrap()
    };
    let cold = open(&[]);
    assert!(
        cold.mount_sidecar(&dir, cap, &build, &dir)
            .unwrap_err()
            .is::<RetrySeek>()
    );
    drop(cold);
    let retry = open(&delta);
    retry.mount_sidecar(&dir, cap, &build, &restored).unwrap();
    let mut machine = BootMachine {
        parts: vec![],
        checks: Box::new(Vec::new),
        rt: None,
        presents: 0,
        shelf: Shelf::new(&dir, build.clone()),
        hash: Box::new(|| lockstep::RamHash {
            total: 2,
            chunks: vec![2],
        }),
    };
    let frame = retry.take_keyframe().unwrap();
    assert!(
        retry
            .restore_keyframe(frame, &mut machine, None)
            .unwrap_err()
            .is::<RetrySeek>()
    );
    assert!(
        !dir.join("cache")
            .join(id)
            .join(format!("{}.h2keys", build.id()))
            .exists()
    );
    let recovered = open(&[]);
    recovered.mount_sidecar(&dir, cap, &build, &dir).unwrap();
    assert!(recovered.take_keyframe().is_none());
    drop((retry, recovered));
    std::fs::remove_dir_all(dir).unwrap();
}
