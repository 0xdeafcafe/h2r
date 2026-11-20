use super::*;
use agent_proto::theater as api;
use runtime::guest::Permissions;
use serde_json::json;

fn arena() -> runtime::Guest {
    let mut g = runtime::Guest::new();
    let lockstep::SimRegion::Indirect { pointer, len } = lockstep::SimRegion::HALO2 else {
        unreachable!()
    };
    g.map_zeroed(pointer & !4095, 4096, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x800000, len, Permissions::RW).unwrap();
    g.set_u32(pointer, 0x800000).unwrap();
    g
}

#[test]
fn theater_replays_checkpoints_reports_first_divergence_and_registers_controls() {
    let dir = std::env::temp_dir().join(format!("film-boot-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("fixture.h2film");
    let facts = host_facts;
    let opts = options(&dir);
    let rec = Session::open(Some(file.clone()), None, facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let mut g = arena();
    for _ in 0..60 {
        rec.checkpoint(&g);
    }
    rec.finish(90, true).unwrap();
    let open = |facts| {
        Session::open(
            None,
            Some(Replay {
                path: file.clone(),
                render_every: 0,
                speed: 1.,
                seek_tick: 0,
                paused_at_target: false,
                restart: dir.join("restart"),
                camera: Default::default(),
                window_closed: None,
            }),
            facts,
            b"xbe",
            &opts,
        )
    };
    let wrong = open(Facts {
        stamp: "other image".into(),
        ..facts()
    });
    assert!(wrong.is_err_and(|e| e.to_string().contains("image stamp differs")));
    let replay = || open(facts()).unwrap().unwrap();
    let matching = replay();
    for _ in 0..60 {
        matching.checkpoint(&g);
    }
    matching.finish(90, true).unwrap();
    assert!(matching.lock().complete);
    let player = replay();
    let a = agent::Agent::new();
    player.add_methods(&a);
    let (send, read) = std::sync::mpsc::sync_channel(32);
    let client = a.connect(send);
    struct Empty;
    impl agent::Reader for Empty {
        fn status(&mut self) -> agent::GameStatus {
            Default::default()
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(vec![])
        }
    }
    let call = |method: &str, p: Value| {
        a.submit(
            client,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":p}).to_string(),
        );
        a.poll_theater(&mut Empty);
        serde_json::from_str::<Value>(&read.try_recv().unwrap()).unwrap()
    };
    let listed = call("session.methods", json!({}));
    for method in api::METHODS {
        assert!(
            listed["result"]
                .as_array()
                .unwrap()
                .contains(&json!(method))
        );
    }
    assert_eq!(call(api::PAUSE, json!({}))["result"]["paused"], true);
    assert_eq!(call(api::PLAY, json!({}))["result"]["paused"], false);
    assert_eq!(
        call(api::SPEED, json!({"speed":0.5}))["result"]["speed"],
        0.5
    );
    assert!(call(api::SPEED, json!({"speed":5}))["error"].is_object());
    assert_eq!(
        call(api::CAMERA, json!({"mode":"free"}))["error"]["message"],
        "unsupported film camera mode"
    );
    let camera = match &player.lock().mode {
        Mode::Replay(_, selected) => selected.camera.clone(),
        Mode::Record(_) => unreachable!(),
    };
    camera.host_seats(&[4, 7]);
    let chosen = call(api::CAMERA, json!({"mode":"first_person", "seat":7}));
    assert_eq!(chosen["result"]["camera"], "first_person");
    assert_eq!(chosen["result"]["camera_seat"], 7);
    assert_eq!(camera.requested_seat(), Some(7));
    assert!(call(api::CAMERA, json!({"mode":"first_person","seat":3}))["error"].is_object());
    assert_eq!(
        call(api::CAMERA, json!({"mode":"default"}))["result"]["camera"],
        "default"
    );
    for _ in 0..30 {
        player.checkpoint(&g);
    }
    assert!(player.lock().failure.is_none());
    g.set_u32(0x800000, 7).unwrap();
    for _ in 0..60 {
        player.checkpoint(&g);
    }
    assert!(
        player
            .finish(90, true)
            .unwrap_err()
            .to_string()
            .starts_with("first divergence at tick 60")
    );
    let restarted = replay();
    assert_eq!(restarted.lock().tick, 0);
    restarted.add_methods(&a);
    assert_eq!(
        call(api::SEEK, json!({"tick":20}))["result"]["seeking"],
        true
    );
    assert_eq!(restarted.lock().restart, Some(20));
    assert!(call(api::SEEK, json!({"tick":61}))["error"].is_object());
    std::fs::remove_dir_all(dir).unwrap();
}

fn host_facts() -> Facts {
    Facts {
        stamp: "fixture".into(),
        title_id: 2,
        epoch: 0,
        light: true,
        bots: false,
        address: "127.0.0.1:3074".into(),
        live_service: false,
    }
}

fn options(data: &std::path::Path) -> crate::RunOpts {
    crate::RunOpts {
        disc: PathBuf::new(),
        data_root: data.to_owned(),
        max_slices: 90,
        xapi: true,
        transfers_per_slice: 5000,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        movies: crate::movies_glue::Movies::Off,
    }
}

#[test]
fn a_film_only_host_propagates_publication_failure_at_teardown() {
    let dir = std::env::temp_dir().join(format!("film-required-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let mut film = Session::open(Some(dir.join("game.h2film")), None, host_facts(), b"xbe", &options(&dir))
        .unwrap().unwrap();
    Arc::get_mut(&mut film).unwrap().require_upload = true;
    *film.upload.lock().unwrap() =
        Some(std::thread::spawn(|| anyhow::bail!("fixture publication failed")));
    assert!(film.finish(0, true).is_err_and(|e| e.to_string().contains("fixture publication failed")));
    drop(film);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_completed_host_match_is_listed_and_uploads_under_its_lease() {
    use std::{
        io::{BufRead, BufReader, Cursor, Read, Write},
        net::TcpListener,
        time::Duration,
    };

    let dir = std::env::temp_dir().join(format!("film-upload-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("match.h2film");
    let session = Session::open(Some(file.clone()), None, host_facts(), b"xbe", &options(&dir))
        .unwrap()
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let target = UploadTarget::from_env(Some("hl-fixture"), |key| match key {
        "H2_HOST_LIVE_URL" => Some(base.clone()),
        "H2_HOST_TOKEN" => Some("test-token".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    {
        let mut state = session.lock();
        state.header.meta.map = "lockout".into();
        state.header.lease_id = Some("hl-fixture".into());
        let Mode::Record(rec) = &mut state.mode else {
            unreachable!();
        };
        rec.upload = Some(target);
    }
    let guest = arena();
    for _ in 0..30 {
        session.checkpoint(&guest);
    }
    session.finish(60, false).unwrap();
    assert!(session.upload.lock().unwrap().is_none());

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim_end(), "POST /v1/games/hl-fixture/film HTTP/1.1");
        let mut length = None;
        let mut authorised = false;
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line).unwrap();
            if bytes == 0 || line.trim_end().is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse().unwrap());
            }
            if name.eq_ignore_ascii_case("authorization") {
                authorised = value.trim() == "Bearer test-token";
            }
        }
        assert!(authorised);
        let mut body = vec![0; length.unwrap()];
        reader.read_exact(&mut body).unwrap();
        let uploaded = Reader::new(Cursor::new(body)).unwrap();
        assert!(uploaded.finalized);
        assert_eq!(uploaded.header.meta.map, "lockout");
        assert_eq!(uploaded.header.meta.length_ticks, 30);
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    // A disconnect can close the match without the retail game-end latch.
    // Feed the observer the same final event as Telemetry::close_film.
    let quit = crate::telemetry::GameEvent::new(
        "hl-fixture",
        live_proto::game::GAME_ENDED,
        serde_json::json!({"tick": 30, "reason": "quit", "players": []}),
    );
    let observer = session.film_observer().unwrap();
    observer.event(&quit, 30).unwrap();
    assert!(!observer.active(), "a second match in the same batch must not enter this film");
    session.finish(90, false).unwrap();
    assert!(session.upload.lock().unwrap().is_some());
    session.finish(90, true).unwrap();
    assert!(session.upload.lock().unwrap().is_none());
    server.join().unwrap();

    let entries = film::Library::open(&dir).unwrap().list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, file);
    assert_eq!(entries[0].lease_id.as_deref(), Some("hl-fixture"));
    assert!(entries[0].playable_here("fixture"));
    assert!(!entries[0].playable_here("other image"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn seek_restores_machine_and_replays_only_the_remaining_ticks() {
    let dir = std::env::temp_dir().join(format!("engine-seek-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("game.h2film");
    let opts = options(&dir);
    let record = Session::open(Some(file.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap().unwrap();
    let mut machine = crate::snapshot_glue::BootMachine {
        parts: vec![],
        checks: Box::new(Vec::new),
        shelf: crate::snapshot_glue::Shelf::new(&dir, snapshot::BuildKey {
            stamp: "fixture".into(),
            switches: String::new(),
        }),
        hash: Box::new(|| lockstep::RamHash { total: 42, chunks: vec![42] }),
        presents: 1,
        rt: None,
    };
    let guest = arena();
    for _ in 0..30 {
        record.checkpoint(&guest);
    }
    record.try_keyframe(35, 0, &mut machine);
    record.checkpoint(&guest);
    record.finish(40, true).unwrap();

    let replay = Session::open(None, Some(Replay {
        path: file,
        render_every: 0,
        speed: 1.,
        seek_tick: 31,
        paused_at_target: true,
        restart: dir.join("restart"),
        camera: Default::default(),
                window_closed: None,
    }), host_facts(), b"xbe", &opts).unwrap().unwrap();
    let keyframe = replay.take_keyframe().unwrap();
    assert_eq!(keyframe.tick, 30);
    let start = replay.restore_keyframe(keyframe, &mut machine, None).unwrap();
    assert_eq!(start, 35);
    assert_eq!(replay.lock().tick, 30);
    replay.checkpoint(&guest);
    assert!(replay.lock().paused);
    replay.finish(40, true).unwrap();
    assert!(replay.lock().complete);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_film_records_the_consumed_action_and_both_edges_of_a_tickless_pause() {
    use film::blam::{Event, SeatInput, TrackReader};

    let dir = std::env::temp_dir().join(format!("film-local-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("match.h2film");
    let rec = Session::open(
        Some(file.clone()),
        None,
        Facts { light: false, ..host_facts() },
        b"xbe",
        &options(&dir),
    ).unwrap().unwrap();
    let mut guest = arena();
    let ptr = crate::telemetry::layout::GAME_TIME_PTR;
    guest.map_zeroed(ptr & !4095, 4096, Permissions::RW).unwrap();
    guest.set_u32(ptr, 0x800000).unwrap();
    guest.set_u8(0x800000, 1).unwrap();
    // In campaign the retail clock runs no game tick between these edges.
    for paused in [0, 1, 1, 0] {
        guest.set_u8(0x800001, paused).unwrap();
        rec.observe_pause(&guest);
    }

    let action = SeatInput { seat: 13, pad: None, action: Some([51; 92]) };
    rec.record_actions(vec![action]);
    rec.checkpoint(&guest);
    rec.checkpoint(&guest);
    rec.finish(12, true).unwrap();

    let mut track = TrackReader::open(file).unwrap();
    let first = track.step().unwrap().unwrap();
    assert_eq!(first.inputs, vec![action]);
    assert_eq!(first.events, vec![Event::Pause { paused: true }, Event::Pause { paused: false }]);
    let next = track.step().unwrap().unwrap();
    assert!(next.inputs.is_empty());
    assert!(next.events.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
