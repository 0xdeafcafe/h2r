use super::*;
use crate::{FilmMeta, Header, Reader, Source, Writer};
use std::io::Cursor;

fn header(length_ticks: u64) -> Header {
    Header {
        version: 2,
        meta: FilmMeta {
            id: "lease-game".into(),
            map: "example".into(),
            variant: "Slayer".into(),
            started_ms: 12345,
            length_ticks,
            players: vec![],
            source: Source::Host,
            image_stamp: "test-stamp".into(),
        },
        title_id: 1,
        xbe_sha256: "test".into(),
        runtime_stamp: String::new(),
        simulation_hooks: None,
        guest_image: None,
        epoch_100ns: 123,
        lease_id: Some("lease".into()),
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

fn sample(tick: u64) -> Tick {
    let mut pad = [0; 18];
    pad[4] = if tick < 140 { 0xfe } else { 0x01 };
    pad[17] = 0xa5;
    let mut action = [0; 92];
    action[4] = 0x80;
    action[91] = if tick < 140 { 2 } else { 3 };
    let mut inputs = Vec::new();
    if !(4..=10).contains(&tick) {
        inputs.push(SeatInput {
            seat: 0,
            pad: Some(pad),
            action: None,
        });
    }
    if tick > 1 {
        inputs.push(SeatInput {
            seat: 15,
            pad: None,
            action: Some(action),
        });
    }
    Tick {
        tick,
        inputs,
        events: match tick {
            2 => vec![Event::Join {
                player: Seat {
                    seat: 15,
                    kind: SeatKind::Phantom,
                    ..Seat::default()
                },
            }],
            4 => vec![Event::Team { seat: 15, team: 1 }, Event::Quit { seat: 0 }],
            _ => vec![],
        },
        closing_events: vec![],
        post_hash: Some(0xfe00 + tick),
    }
}

fn key(tick: u64) -> Keyframe {
    Keyframe {
        tick,
        slices: tick * 3,
        calls: tick * 2,
        lobby_boundary: tick,
        disk_delta: format!(r#"{{"test":{tick}}}"#).into_bytes(),
        snapshot: vec![0xf1; 999],
        ram_anchor: None,
    }
}

fn scores() -> Summary {
    Summary {
        level: "example".into(),
        game_type: "slayer".into(),
        mode: Mode::SystemLink,
        variant_name: "Slayer".into(),
        duration_ticks: 299,
        dropped_samples: Default::default(),
        players: vec![PlayerSummary {
            seat: 15,
            name: "Spartan".into(),
            xuid: Some(u64::MAX),
            team: 1,
            score: 12,
            medals: [("Double Kill".to_owned(), 3)].into_iter().collect(),
        }],
    }
}

fn encoded() -> Vec<u8> {
    encoded_as(2)
}

fn encoded_as(version: u32) -> Vec<u8> {
    let mut header = header(300);
    header.version = version;
    let mut writer = Writer::new(Cursor::new(Vec::new()), &header).unwrap();
    let mut track = TrackWriter::default();
    for tick in 1..=300 {
        track.push(&mut writer, sample(tick)).unwrap();
        if matches!(tick, 6 | 192) {
            track.keyframe(&mut writer, &key(tick)).unwrap();
        }
    }
    let descriptor = MatchHeader {
        map: MapIdentity {
            name: "example".into(),
            origin: "mod".into(),
            sha256: "a".repeat(64),
            ..Default::default()
        },
        variant_blob: vec![0, 1, 255],
        ..Default::default()
    };
    track.finish(&mut writer, &descriptor, &scores()).unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn full_round_trip_and_divergence() {
    let mut track = TrackReader::new(Reader::new(Cursor::new(encoded())).unwrap()).unwrap();
    assert_eq!(track.descriptor.variant_blob, [0, 1, 255]);
    assert_eq!(track.summary, scores());
    assert_eq!(track.length_ticks(), 300);
    for t in 1..=300 {
        let tick = track.step().unwrap().unwrap();
        assert_eq!(tick, sample(t));
        tick.verify(Some(t + 0xfe00)).unwrap();
        let error = tick.verify(None).unwrap_err().to_string();
        assert!(error.contains(&format!("tick {t}:")));
    }
    assert!(track.step().unwrap().is_none());
}

#[test]
fn seek_flushes_partial_blocks_and_resumes_after_restored_tick() {
    let mut track = TrackReader::new(Reader::new(Cursor::new(encoded())).unwrap()).unwrap();
    for (target, nearest) in [
        (0, None),
        (5, None),
        (6, Some(6)),
        (7, Some(6)),
        (191, Some(6)),
        (192, Some(192)),
        (300, Some(192)),
        (1, None),
    ] {
        assert_eq!(track.seek(target).unwrap(), nearest.map(key));
        assert_eq!(track.keyframe_offset().is_some(), nearest.is_some());
        assert_eq!(
            track.step().unwrap().unwrap(),
            sample(nearest.unwrap_or(0) + 1)
        );
    }
    assert!(track.seek(301).is_err());
}

#[test]
fn ring_cursor_resumes_inside_a_tick_block_without_a_file_keyframe() {
    for version in [2, 3] {
        let bytes = encoded_as(version);
        let reader = || TrackReader::new(Reader::new(Cursor::new(&bytes)).unwrap()).unwrap();
        let mut linear = reader();
        let mut expected = Vec::new();
        while let Some(tick) = linear.step().unwrap() {
            expected.push(tick);
        }
        let mut track = reader();
        for tick in [0, 83, 6, 192, 300, 12, 299, 83] {
            track.resume_after_tick(tick).unwrap();
            assert_eq!(track.keyframe_offset(), None);
            assert_eq!(track.step().unwrap().as_ref(), expected.get(tick as usize));
        }
        assert!(track.resume_after_tick(301).is_err());
    }
}

#[test]
fn menu_skips_corrupt_input_and_playback_reports_it() {
    let mut data = encoded();
    let r = Reader::new(Cursor::new(data.as_slice())).unwrap();
    let block = r.entries.iter().find(|e| e.tag == INPT).unwrap();
    let start = block.offset as usize + 8;
    data[start] ^= 0xff;
    let mut r = Reader::new(Cursor::new(data)).unwrap();
    assert_eq!(summary(&mut r).unwrap(), scores());
    let mut track = TrackReader::new(r).unwrap();
    assert!(track.step().is_err());
    assert!(
        track.step().is_err(),
        "failed step advanced past damaged input"
    );
}

#[test]
fn rejects_truncated_container_and_future_version() {
    let data = encoded();
    for n in [0, 1, 10, data.len() / 2, data.len() - 17, data.len() - 1] {
        assert!(Reader::new(Cursor::new(&data[..n])).is_err());
    }
    let mut h = header(0);
    h.version = 99;
    let frame = zstd::stream::encode_all(serde_json::to_vec(&h).unwrap().as_slice(), 1).unwrap();
    let mut raw = Vec::from(crate::HEAD);
    raw.extend((frame.len() as u32).to_le_bytes());
    raw.extend(frame);
    let err = Reader::new(Cursor::new(raw)).err().unwrap();
    assert!(err.to_string().contains("unsupported film version 99"));
}

#[test]
fn malformed_input_vectors_are_bounded_and_rejected() {
    for data in [
        vec![1, 1, 0, 0, 0],
        vec![1, 1, 2, 0, 0],
        vec![1, 1, 1, 1, 4, 0],
        vec![1, 1, 1, 0, 1, 0],
        vec![1, 1, 1, 0, 0x80],
        vec![1, 1, 1, 0, 0, 0],
        vec![0xff; 16],
        vec![0x81, 0, 1, 1, 0, 0],
    ] {
        assert!(ticks::decode(&data, 1, 1).is_err(), "{data:?}");
    }
    let mut tick = sample(1);
    tick.inputs.push(tick.inputs[0]);
    assert!(ticks::encode(&[tick]).is_err());
}

#[test]
fn long_constant_input_uses_one_run_and_separate_hashes() {
    let mut block: Vec<_> = (1..=128)
        .map(|n| Tick {
            tick: n,
            ..sample(1)
        })
        .collect();
    for tick in &mut block {
        tick.post_hash = None;
    }
    let encoded = ticks::encode(&block).unwrap();
    assert!(encoded.len() < 150, "{}", encoded.len());
    assert_eq!(
        ticks::decode(&encoded, 1, 128)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        block
    );
}

#[test]
fn a_directory_cannot_omit_a_tick_or_reference_another_section() {
    for (first, wrong_offset) in [(2, false), (1, true)] {
        let mut writer = Writer::new(Cursor::new(Vec::new()), &header(1)).unwrap();
        let raw = ticks::encode(&[sample(1)]).unwrap();
        writer.chunk_bytes(INPT, &raw).unwrap();
        let offset = writer.index.last().unwrap().offset;
        writer.chunk(DESC, &MatchHeader::default()).unwrap();
        writer.chunk(SUMM, &Summary::default()).unwrap();
        writer.chunk(TIDX, &serde_json::json!({
            "blocks": [{ "tick": first, "count": 1, "offset": if wrong_offset { 0 } else { offset },
                "sha256": crate::xbe_digest(&raw) }],
            "keyframes": []
        })).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let reader = Reader::new(Cursor::new(bytes)).unwrap();
        assert!(TrackReader::new(reader).is_err());
    }
}

#[test]
fn unified_time_chunks_restore_inputs_and_skip_future_payload_types() {
    let mut source: Vec<_> = (1..=8).map(sample).collect();
    source
        .last_mut()
        .unwrap()
        .closing_events
        .push(Event::Quit { seat: 15 });
    let mut extra = vec![unified::TickRecords::default(); source.len()];
    extra[1].during = vec![
        RecordData::pose(vec![0xff, 3]),
        RecordData {
            tag: 0xd9,
            payload: vec![0xff; 50],
        },
        RecordData::stats_event(br#"{"kind":"test","data":{}}"#.to_vec()),
        RecordData::marker("Overtime"),
    ];
    let raw = unified::encode(&source, &extra).unwrap();
    let replay = unified::decode(&raw, 1, source.len()).unwrap();
    assert_eq!(Vec::from(replay), source);
    let records: Vec<_> = stream::Chunk::parse(&raw)
        .unwrap()
        .records()
        .map(|r| r.unwrap())
        .filter(|r| r.tick == 2)
        .collect();
    assert!(
        records
            .iter()
            .any(|r| r.tag == stream::EVENT && r.payload[0] == stream::LIFECYCLE_EVENT)
    );
    assert!(
        records
            .iter()
            .any(|r| r.tag == stream::EVENT && r.payload[0] == stream::STATS_EVENT)
    );
    assert!(
        records
            .iter()
            .any(|r| r.tag == stream::MARKER && r.payload == b"Overtime")
    );
}

#[test]
fn final_score_can_follow_a_keyframe_without_reopening_an_older_time_chunk() {
    let mut h = header(5);
    h.version = 3;
    let mut writer = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    let mut track = TrackWriter::default();
    assert!(track.begins_chunk());
    for n in 1..=5 {
        track
            .push_with_records(&mut writer, sample(n), vec![RecordData::marker("Chapter")])
            .unwrap();
        if n == 2 || n == 5 {
            track.keyframe(&mut writer, &key(n)).unwrap();
            assert!(track.begins_chunk());
        }
    }
    let final_event = live_proto::film::events::Record {
        tick: 5,
        event: live_proto::film::events::Event::GameEnded(live_proto::game::GameEnded {
            tick: 5,
            reason: "finished".into(),
            winner: None,
            players: Vec::new(),
        }),
    }
    .encode()
    .unwrap();
    let final_record = RecordData::stats_event(final_event);
    track.append_to_last(vec![final_record.clone()]).unwrap();
    track
        .finish(&mut writer, &MatchHeader::default(), &scores())
        .unwrap();
    let mut reader = Reader::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap();
    let blocks: Vec<_> = reader
        .entries
        .iter()
        .filter(|e| e.tag == stream::TAG)
        .cloned()
        .collect();
    assert_eq!(blocks.len(), 2);
    let raw = reader.read_bytes(&blocks[1]).unwrap();
    let chunk = stream::Chunk::parse(&raw).unwrap();
    assert_eq!((chunk.first_tick, chunk.tick_count), (3, 3));
    let end: Vec<_> = chunk
        .records()
        .map(|r| r.unwrap())
        .filter(|r| r.tick == 5)
        .collect();
    let hash = end.iter().position(|r| r.tag == stream::HASH).unwrap();
    let scoreboard = end
        .iter()
        .position(|r| r.tag == stream::EVENT && r.payload == final_record.payload.as_slice())
        .unwrap();
    assert!(scoreboard > hash);
    let mut replay = TrackReader::new(reader).unwrap();
    assert_eq!(replay.seek(4).unwrap(), Some(key(2)));
    assert_eq!(replay.step().unwrap(), Some(sample(3)));
    assert_eq!(replay.seek(5).unwrap(), Some(key(5)));
    assert!(replay.step().unwrap().is_none());
}

#[test]
fn inactive_poses_only_need_periodic_scene_checkpoints() {
    let mut h = header(18);
    h.version = 3;
    let mut writer = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    let mut track = TrackWriter::default();
    for n in 1..=18 {
        // Force very short chunks. The chunk count still bounds HTTP seeks.
        let checkpoint = track.needs_pose_baseline();
        assert_eq!(checkpoint, n == 1 || n == 17);
        let records = if checkpoint {
            vec![RecordData::pose(vec![0, 0, 0, 0])]
        } else {
            vec![]
        };
        track
            .push_with_records(&mut writer, sample(n), records)
            .unwrap();
        track.keyframe(&mut writer, &key(n)).unwrap();
    }
    track
        .finish(&mut writer, &MatchHeader::default(), &Summary::default())
        .unwrap();
    let mut reader = Reader::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap();
    let blocks: Vec<_> = reader
        .entries
        .iter()
        .filter(|e| e.tag == stream::TAG)
        .cloned()
        .collect();
    let mut checkpoints = Vec::new();
    for block in blocks {
        let bytes = reader.read_bytes(&block).unwrap();
        for record in stream::Chunk::parse(&bytes).unwrap().records() {
            let record = record.unwrap();
            if record.tag == stream::POSE {
                checkpoints.push(record.tick);
            }
        }
    }
    assert_eq!(checkpoints, [1, 17]);
}

#[test]
fn cold_seek_reads_the_standalone_ram_anchor_and_only_the_targets_disk() {
    let mut header = header(3);
    header.version = 3;
    let mut writer = Writer::new(Cursor::new(Vec::new()), &header).unwrap();
    let mut track = TrackWriter::default();
    for tick in 1..=3 {
        track.push(&mut writer, sample(tick)).unwrap();
        let mut frame = key(tick);
        frame.snapshot = vec![tick as u8; 7];
        if tick == 3 {
            assert!(
                track
                    .keyframe_against(&mut writer, &frame, Some(2))
                    .is_err()
            );
        }
        track
            .keyframe_against(&mut writer, &frame, (tick > 1).then_some(1))
            .unwrap();
    }
    track
        .finish(&mut writer, &MatchHeader::default(), &scores())
        .unwrap();
    let file = Reader::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap();
    let mut reader = TrackReader::new(file).unwrap();
    let frame = reader.seek(3).unwrap().unwrap();
    assert_eq!(frame.ram_anchor, Some((1, vec![1; 7])));
    assert_eq!(frame.snapshot, vec![3; 7]);
    assert_eq!(frame.disk_delta, key(3).disk_delta);
    assert_eq!(reader.disk_delta_at(3).unwrap(), key(3).disk_delta);
    assert!(reader.disk_delta_at(0).unwrap().is_empty());
    assert!(reader.seek(1).unwrap().unwrap().ram_anchor.is_none());
    assert_eq!(reader.step().unwrap(), Some(sample(2)));
}

#[test]
fn match_clock_anchors_cover_stalls_reverts_and_inactive_ticks() {
    let clocks = [
        None,
        None,
        Some(100),
        Some(101),
        Some(101),
        Some(102),
        Some(103),
        Some(7),
        Some(8),
        None,
        None,
        Some(0),
    ];
    let mut h = header(clocks.len() as u64);
    h.version = 3;
    let mut out = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    let mut writer = TrackWriter::default();
    for (i, clock) in clocks.iter().copied().enumerate() {
        let tick = i as u64 + 1;
        writer
            .push_clocked(&mut out, sample(tick), vec![], clock)
            .unwrap();
        if tick == 6 {
            writer.keyframe(&mut out, &key(tick)).unwrap();
        }
    }
    writer
        .finish(&mut out, &MatchHeader::default(), &Summary::default())
        .unwrap();
    let mut reader = Reader::new(Cursor::new(out.finish().unwrap().into_inner())).unwrap();
    let entries: Vec<_> = reader
        .entries
        .iter()
        .filter(|e| e.tag == stream::TAG)
        .cloned()
        .collect();
    let mut anchors = Vec::new();
    for entry in entries {
        let raw = reader.read_bytes(&entry).unwrap();
        for record in stream::Chunk::parse(&raw).unwrap().records() {
            let record = record.unwrap();
            if record.tag == stream::CLOCK {
                anchors.push((record.tick, stream::decode_clock(record.payload).unwrap()));
            }
        }
    }
    assert_eq!(
        anchors,
        [
            (1, None),
            (3, Some(100)),
            (5, Some(101)),
            (7, Some(103)),
            (8, Some(7)),
            (10, None),
            (12, Some(0))
        ]
    );
    assert!(stream::decode_clock(&[0x80, 0]).is_err());
    assert!(stream::decode_clock(&[1, 2]).is_err());
}

#[test]
fn identical_v3_captures_have_identical_bytes() {
    let a = encoded_as(3);
    let b = encoded_as(3);
    // Include the front, compressed time chunks, snapshots and both directories.
    assert_eq!(a.len(), b.len());
    assert!(a == b);
}

#[test]
fn cidx_seeks_return_the_same_inputs_events_and_hashes_as_linear_playback() {
    let bytes = encoded_as(3);
    let mut linear = TrackReader::new(Reader::new(Cursor::new(&bytes)).unwrap()).unwrap();
    let mut expected = Vec::new();
    while let Some(tick) = linear.step().unwrap() {
        expected.push(tick);
    }
    let mut public = crate::range::RangeReader::new(Cursor::new(&bytes)).unwrap();
    // Seek in both directions, across a compression boundary (134/135) and
    // across a machine keyframe (192/193). Each fetched chunk starts from
    // an empty input dictionary even when seats were held between chunks.
    for (from, to) in [(182, 197), (128, 141), (6, 7), (300, 300)] {
        let rows = public.directory.covering(from, to).to_vec();
        assert_eq!(rows.len(), if from == to { 1 } else { 2 });
        let mut actual = Vec::new();
        for row in rows {
            let index = public
                .directory
                .chunks
                .binary_search_by_key(&row.first, |c| c.first)
                .unwrap();
            let raw = public.chunk(index).unwrap();
            let ticks =
                unified::decode(&raw, row.first, (row.last - row.first + 1) as usize).unwrap();
            actual.extend(ticks.into_iter().filter(|t| (from..=to).contains(&t.tick)));
        }
        assert_eq!(
            actual.as_slice(),
            &expected[(from - 1) as usize..to as usize]
        );
    }
}

fn join_with_name(bytes: usize) -> Event {
    Event::Join {
        player: Seat {
            name: "a".repeat(bytes),
            ..Default::default()
        },
    }
}

#[test]
fn closing_event_limits_are_checked_at_admission_and_failed_appends_are_atomic() {
    let mut h = header(2);
    h.version = 3;
    let mut out = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    let mut writer = TrackWriter::default();
    let quit = Event::Quit { seat: 0 };
    let oversized = join_with_name(ticks::MAX_EVENT_BYTES);
    let mut first = sample(1);
    first.closing_events = vec![oversized.clone()];
    assert!(writer.push(&mut out, first.clone()).is_err());
    first.closing_events = vec![quit.clone(); 255];
    writer.push(&mut out, first.clone()).unwrap();
    assert!(
        writer
            .closing_events(vec![quit.clone(), quit.clone()])
            .is_err()
    );
    assert!(
        writer
            .closing_events(vec![Event::Quit { seat: 16 }])
            .is_err()
    );
    assert!(writer.closing_events(vec![oversized]).is_err());
    writer.closing_events(vec![quit.clone()]).unwrap();
    first.closing_events.push(quit);
    let mut second = sample(2);
    second.closing_events = vec![join_with_name(40 << 10)];
    writer.push(&mut out, second.clone()).unwrap();
    assert!(
        writer
            .closing_events(second.closing_events.clone())
            .is_err()
    );
    writer
        .finish(&mut out, &MatchHeader::default(), &Summary::default())
        .unwrap();
    let mut track = TrackReader::new(Reader::new(out.finish().unwrap()).unwrap()).unwrap();
    assert_eq!(track.step().unwrap(), Some(first));
    assert_eq!(track.step().unwrap(), Some(second));
}

#[test]
fn closing_events_share_the_chunk_budget_with_pose_and_input_records() {
    let mut h = header(1);
    h.version = 3;
    let mut out = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    let mut writer = TrackWriter::default();
    let mut tick = sample(1);
    let closing = vec![join_with_name(2048)];
    let pose = RecordData::pose(vec![0; stream::MAX_BYTES - 1024]);
    tick.closing_events = closing.clone();
    assert!(
        writer
            .push_with_records(&mut out, tick.clone(), vec![pose.clone()])
            .is_err()
    );
    tick.closing_events.clear();
    writer
        .push_with_records(&mut out, tick.clone(), vec![pose])
        .unwrap();
    assert!(writer.closing_events(closing).is_err());
    writer
        .finish(&mut out, &MatchHeader::default(), &Summary::default())
        .unwrap();
    let mut track = TrackReader::new(Reader::new(out.finish().unwrap()).unwrap()).unwrap();
    assert_eq!(track.step().unwrap(), Some(tick));
}

#[test]
fn private_join_and_migration_round_trip_across_a_keyframe() {
    let mut header = header(2);
    header.version = 3;
    let mut out = Writer::new(Cursor::new(Vec::new()), &header).unwrap();
    let mut recorder = TrackWriter::default();
    recorder.push(&mut out, sample(1)).unwrap();
    recorder.keyframe(&mut out, &key(1)).unwrap();
    let mut last = sample(2);
    if let Event::Join { player } = &mut last.events[0] {
        player.xuid = Some(0xaa74_ef82_f031);
        player.bot_params = Some(serde_json::json!({"observed":"203.0.113.98:3074"}));
    }
    last.closing_events.push(Event::HostMigration {
        host_id: "aa74ef82f031".into(),
    });
    recorder.push(&mut out, last.clone()).unwrap();
    recorder
        .finish(&mut out, &MatchHeader::default(), &scores())
        .unwrap();
    let bytes = out.finish().unwrap().into_inner();
    let mut remote = crate::range::RangeReader::new(Cursor::new(&bytes)).unwrap();
    assert!(
        remote
            .front
            .summary
            .players
            .iter()
            .all(|p| p.xuid.is_none())
    );
    let public = remote.chunk(1).unwrap();
    assert!(live_proto::film::privacy::is_public_tick(&public));
    for field in [
        "host_id",
        "xuid",
        "bot_params",
        "aa74ef82f031",
        "203.0.113.98",
    ] {
        assert!(!public.windows(field.len()).any(|w| w == field.as_bytes()));
    }
    let mut player = TrackReader::new(Reader::new(Cursor::new(&bytes)).unwrap()).unwrap();
    assert_eq!(player.summary, scores());
    assert_eq!(player.seek(1).unwrap(), Some(key(1)));
    assert_eq!(player.step().unwrap(), Some(last));
}
