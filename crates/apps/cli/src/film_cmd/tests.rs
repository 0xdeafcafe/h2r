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
        inputs_in_film: false,
        live_service: false,
        features: String::new(),
        player_features: false,
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

/// A v3 film with a standalone anchor at tick 30 and a delta against it at
/// tick 60.
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
                    firefight: vec![],
                },
                vec![],
                None,
            )
            .unwrap();
        match tick {
            30 => track.keyframe(&mut out, &key(tick)).unwrap(),
            60 => track
                .keyframe_against(&mut out, &key(tick), Some(30))
                .unwrap(),
            _ => {}
        }
    }
    track
        .finish(&mut out, &MatchHeader::default(), &Summary::default())
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
fn film_stat_lists_keyframes_and_anchors() {
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
}

#[test]
fn film_stat_reports_a_film_without_keyframes() {
    let out = film::Writer::new(Cursor::new(Vec::new()), &header(2, 0)).unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let report = stat_bytes(bytes);
    assert!(
        report.contains("keyframes: 0 (0 anchors, 0 deltas, 0 stored bytes)"),
        "{report}"
    );
    assert!(!report.contains("TAPE"), "{report}");
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
                    firefight: vec![],
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
    let films = || vec![film::blam::TrackReader::new(film::Reader::new(Cursor::new(bytes.clone())).unwrap()).unwrap()];
    let report = field_table(films()).unwrap();
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
    let both = field_table(films().into_iter().chain(films()).collect::<Vec<_>>()).unwrap();
    assert!(both.contains("films: 2\nticks: 80\n"), "{both}");
    assert!(both.contains("FIELD yaw words=1 seat_ticks=80 changed=80 "), "{both}");
}

#[test]
fn film_stat_names_an_unsealed_stats_container_rather_than_a_footer_fault() {
    let mut bytes = sample_film();
    // A recording killed mid-game: the container's front and chunks, no
    // footer. `stat` takes the film's path and reads the container beside.
    bytes.truncate(bytes.len() - film::range::FOOTER_BYTES - 16);
    let dir = std::env::temp_dir().join(format!("film-stat-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("killed.h2film");
    std::fs::write(film::file::stats_of(&path), bytes).unwrap();
    let error = format!("{:#}", stat(&path).unwrap_err());
    std::fs::remove_dir_all(dir).unwrap();
    assert!(error.contains("an unsealed stats container"), "{error}");
}
