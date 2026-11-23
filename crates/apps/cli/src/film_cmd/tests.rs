use super::*;
use film::blam::{Keyframe, MatchHeader, Summary, Tick, TrackWriter};
use std::io::Cursor;

fn header(version: u32, length_ticks: u64) -> film::Header {
    film::Header {
        version,
        meta: film::FilmMeta {
            id: "film-stat".into(),
            map: "example".into(),
            variant: "Slayer".into(),
            started_ms: 12345,
            length_ticks,
            players: vec![],
            source: film::Source::Host,
            image_stamp: "test-stamp".into(),
        },
        title_id: 1,
        xbe_sha256: "test".into(),
        runtime_stamp: String::new(),
        simulation_hooks: None,
        guest_image: None,
        epoch_100ns: 123,
        lease_id: None,
        game_id: None,
        xnet_addr: "127.0.0.1:1234".into(),
        transfers_per_slice: 1000,
        lobby: false,
        lobby_machine_start: false,
        host_bots: false,
        telemetry: false,
        telemetry_hooks: None,
        render_hooks: None,
        live_service: false,
        features: String::new(),
        player_features: false,
        tape_id: None,
    }
}

fn key(tick: u64) -> Keyframe {
    Keyframe {
        tick,
        slices: tick * 2,
        calls: tick * 2,
        lobby_boundary: 0,
        disk_delta: Vec::new(),
        snapshot: vec![0xf1; 999],
        ram_anchor: None,
    }
}

/// A v3 film with a standalone anchor at tick 30, a delta against it at
/// tick 60 and two recorded capture boundaries.
fn sample_film() -> Vec<u8> {
    let mut out = film::Writer::new(Cursor::new(Vec::new()), &header(3, 60)).unwrap();
    let mut track = TrackWriter::default();
    for tick in 1..=60u64 {
        track
            .push_clocked(
                &mut out,
                Tick {
                    tick,
                    inputs: vec![],
                    events: vec![],
                    closing_events: vec![],
                    post_hash: None,
                    forge: vec![],
                },
                vec![],
                None,
            )
            .unwrap();
        match tick {
            30 => track.keyframe(&mut out, &key(tick)).unwrap(),
            45 => out
                .chunk(
                    film::MARK,
                    &film::tape::Mark::D3dBarrier {
                        slices: 91,
                        tick,
                        saved: false,
                    },
                )
                .unwrap(),
            60 => {
                out.chunk(
                    film::MARK,
                    &film::tape::Mark::D3dBarrier {
                        slices: 130,
                        tick,
                        saved: true,
                    },
                )
                .unwrap();
                track
                    .keyframe_against(&mut out, &key(tick), Some(30))
                    .unwrap();
            }
            _ => {}
        }
    }
    track
        .finish(&mut out, &MatchHeader::default(), &Summary::default())
        .unwrap();
    out.chunk(
        film::MARK,
        &film::tape::Mark::End {
            slices: 140,
            tick: 60,
            calls: 0,
        },
    )
    .unwrap();
    out.finish().unwrap().into_inner()
}

fn stat_bytes(bytes: Vec<u8>) -> String {
    let dir = std::env::temp_dir().join(format!("film-stat-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("match.h2film");
    std::fs::write(&path, bytes).unwrap();
    let report = stat(&path).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    report
}

#[test]
fn film_stat_lists_keyframes_anchors_and_capture_points() {
    let report = stat_bytes(sample_film());
    assert!(report.contains("version: 3"), "{report}");
    assert!(report.contains("source: host"), "{report}");
    assert!(report.contains("length_ticks: 60\n"), "{report}");
    assert!(
        report.contains("keyframes: 2 (1 anchors, 1 deltas,"),
        "{report}"
    );
    let anchor = report
        .lines()
        .find(|line| line.starts_with("KEYF tick=30 "))
        .unwrap();
    assert!(anchor.starts_with("KEYF tick=30 stored="), "{anchor}");
    assert!(anchor.ends_with(" anchor"), "{anchor}");
    let delta = report
        .lines()
        .find(|line| line.starts_with("KEYF tick=60 "))
        .unwrap();
    assert!(delta.ends_with(" delta anchor=30"), "{delta}");
    assert!(
        report.contains("capture_points: 2 (1 saved, 1 failed)"),
        "{report}"
    );
    assert!(
        report.contains("d3d_barrier tick=45 slices=91 saved=false"),
        "{report}"
    );
    assert!(
        report.contains("d3d_barrier tick=60 slices=130 saved=true"),
        "{report}"
    );
}

#[test]
fn film_stat_scans_keyframes_when_the_directory_is_unwritten() {
    let mut out = film::Writer::new(Cursor::new(Vec::new()), &header(2, 7)).unwrap();
    // The keyframe payload's own tick varint: tick 7, cursors 14, 21, 28,
    // an empty disk delta and a one-byte snapshot.
    out.chunk_bytes(film::KEYF, &[7, 14, 21, 28, 0, 0xf1])
        .unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let report = stat_bytes(bytes);
    assert!(
        report.contains("keyframes: 1 (0 anchors, 0 deltas,"),
        "{report}"
    );
    let row = report
        .lines()
        .find(|line| line.starts_with("KEYF tick=7 "))
        .unwrap();
    assert!(row.ends_with(" anchor=unknown"), "{row}");
    assert!(
        report.contains("capture_points: 0 (0 saved, 0 failed)"),
        "{report}"
    );
}

#[test]
fn film_stat_reports_a_film_without_keyframes_or_barriers() {
    let out = film::Writer::new(Cursor::new(Vec::new()), &header(2, 0)).unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let report = stat_bytes(bytes);
    assert!(
        report.contains("keyframes: 0 (0 anchors, 0 deltas, 0 stored bytes)"),
        "{report}"
    );
    assert!(
        report.contains("capture_points: 0 (0 saved, 0 failed)"),
        "{report}"
    );
}

#[test]
fn film_stat_counts_tape_batches_bytes_and_operations() {
    let mut header = header(2, 0);
    header.meta.source = film::Source::Local;
    let mut out = film::Writer::new(Cursor::new(Vec::new()), &header).unwrap();
    let call = |index, op: &str| Call {
        index,
        op: op.into(),
        args: vec![7],
        thread: 1,
        result: Ok(0),
        outputs: vec![vec![23; 8]],
    };
    let first = vec![call(1, "Select"), call(2, "Htonl")];
    let pad = vec![call(3, "XInputGetState")];
    let second = vec![call(4, "Select")];
    // One batch holds pad and socket calls in call order.
    let all: Vec<Call> = first.iter().chain(&pad).chain(&second).cloned().collect();
    out.chunk_bytes(film::tape::TAPE, &film::tape::encode_batch(&all).unwrap())
        .unwrap();
    out.chunk(
        PDEV,
        &film::tape::PortChange {
            sync: 0,
            ports: [None; 4],
        },
    )
    .unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let reader = film::Reader::new(Cursor::new(&bytes)).unwrap();
    let stored: u64 = reader
        .entries
        .iter()
        .filter(|e| e.tag == film::tape::TAPE)
        .map(|e| u64::from(e.size) + 8)
        .sum();
    let json = |batch: &[Call]| -> u64 {
        batch
            .iter()
            .map(|c| serde_json::to_vec(c).unwrap().len() as u64 + 1)
            .sum()
    };
    let binary = |batch: &[Call]| film::tape::encode_batch(batch).unwrap().len();
    let net = format!(
        "NETR calls=3 json={} binary={}\n  \"Htonl\" calls=1\n  \"Select\" calls=2\n",
        json(&first) + json(&second),
        binary(&[first.clone(), second.clone()].concat()),
    );
    let pad = format!(
        "PADR calls=1 json={} binary={}\n  \"XInputGetState\" calls=1\n",
        json(&pad),
        binary(&pad),
    );
    let report = stat_bytes(bytes);
    assert!(report.contains("source: local"), "{report}");
    assert!(
        report.contains(&format!(
            "TAPE calls=4 inflated={} stored={stored} chunks=1\n",
            binary(&all)
        )),
        "{report}"
    );
    assert!(report.contains(&net), "{report}");
    assert!(report.contains(&pad), "{report}");
    assert!(report.contains("RAND calls=0 json=0 binary=0"), "{report}");
    assert!(report.contains("PDEV calls=0 inflated="), "{report}");
    assert!(!report.contains("outputs"), "{report}");
}

/// 40 ticks of one seat: it turns 0.25 more every tick and presses a
/// button on tick 11 and holds it.
fn actions_film() -> Vec<u8> {
    let mut out = film::Writer::new(Cursor::new(Vec::new()), &header(2, 40)).unwrap();
    let mut track = TrackWriter::default();
    for tick in 1..=40u64 {
        let mut action = [0u8; 92];
        action[4..8].copy_from_slice(&(tick as f32 * 0.25).to_bits().to_le_bytes());
        if tick >= 11 {
            action[0] = 4;
        }
        track
            .push(
                &mut out,
                Tick {
                    tick,
                    inputs: vec![film::blam::SeatInput { seat: 0, pad: None, action: Some(action) }],
                    events: vec![],
                    closing_events: vec![],
                    post_hash: None,
                    forge: vec![],
                },
            )
            .unwrap();
    }
    let descriptor = MatchHeader {
        map: film::blam::MapIdentity {
            name: "example".into(),
            origin: "mod".into(),
            sha256: "a".repeat(64),
            ..Default::default()
        },
        variant_blob: vec![0],
        ..Default::default()
    };
    track.finish(&mut out, &descriptor, &Summary::default()).unwrap();
    out.finish().unwrap().into_inner()
}

#[test]
fn film_stat_fields_counts_changes_and_prices_each_predictor() {
    let bytes = actions_film();
    let films = || vec![film::Reader::new(Cursor::new(bytes.clone())).unwrap()];
    let report = fields(films()).unwrap();
    assert!(report.contains("films: 1\nticks: 40\n"), "{report}");
    assert!(
        report.contains("FIELD yaw words=1 seat_ticks=40 changed=40 (100.0%) seat_chunks=1 stored_chunks=1 "),
        "{report}"
    );
    assert!(
        report.contains("FIELD buttons words=1 seat_ticks=40 changed=1 (2.5%) seat_chunks=1 stored_chunks=1 "),
        "{report}"
    );
    // A field that never leaves its start is stored nowhere.
    assert!(
        report.contains("FIELD tail words=1 seat_ticks=40 changed=0 (0.0%) seat_chunks=1 stored_chunks=0 "),
        "{report}"
    );
    // Two films are one report.
    let both = fields([films(), films()].concat()).unwrap();
    assert!(both.contains("films: 2\nticks: 80\n"), "{both}");
    assert!(both.contains("FIELD yaw words=1 seat_ticks=80 changed=80 "), "{both}");
}
