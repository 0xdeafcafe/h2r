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
                track.keyframe_against(&mut out, &key(tick), Some(30)).unwrap();
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
    assert!(report.contains("length_ticks: 60 (2.0 s)"), "{report}");
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
    assert!(report.contains("capture_points: 2 (1 saved, 1 failed)"), "{report}");
    assert!(report.contains("d3d_barrier tick=45 slices=91 saved=false"), "{report}");
    assert!(report.contains("d3d_barrier tick=60 slices=130 saved=true"), "{report}");
}

#[test]
fn film_stat_scans_keyframes_when_the_directory_is_unwritten() {
    let mut out = film::Writer::new(Cursor::new(Vec::new()), &header(2, 7)).unwrap();
    // The keyframe payload's own tick varint: tick 7, cursors 14, 21, 28,
    // an empty disk delta and a one-byte snapshot.
    out.chunk_bytes(film::KEYF, &[7, 14, 21, 28, 0, 0xf1]).unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let report = stat_bytes(bytes);
    assert!(report.contains("keyframes: 1 (0 anchors, 0 deltas,"), "{report}");
    let row = report
        .lines()
        .find(|line| line.starts_with("KEYF tick=7 "))
        .unwrap();
    assert!(row.ends_with(" anchor=unknown"), "{row}");
    assert!(report.contains("capture_points: 0 (0 saved, 0 failed)"), "{report}");
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
    assert!(report.contains("capture_points: 0 (0 saved, 0 failed)"), "{report}");
}
