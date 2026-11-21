mod capture_points;
mod pad_inputs;
mod sidecar_restore;

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
    let open = |facts, render_every, unpaced| {
        Session::open(
            None,
            Some(Replay {
                path: file.clone(),
                cache: None,
                unpaced,
                render_every,
                speed: 1.,
                seek_tick: 0,
                paused_at_target: false,
                presentation: Presentation::default(),
                restart: dir.join("restart"),
                camera: Default::default(),
                window_closed: None,
            }),
            facts,
            b"xbe",
            &opts,
        )
    };
    let wrong = open(
        Facts { stamp: "other image".into(), ..facts() },
        0,
        false,
    );
    assert!(wrong.is_err_and(|e| e.to_string().contains("image stamp differs")));
    let replay = || open(facts(), 0, false).unwrap().unwrap();
    let matching = open(facts(), 0, true).unwrap().unwrap();
    assert!(matching.finish(0, true).is_err_and(|e| {
        e.to_string().contains("playback stopped before the end")
    }));
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
        "playback has no rendered view"
    );
    let rendered = open(facts(), 1, false).unwrap().unwrap();
    rendered.add_methods(&a);
    let camera = match &rendered.lock().mode {
        Mode::Replay(_, replay) => replay.camera.clone(),
        Mode::Record(_) => unreachable!(),
    };
    camera.host_seats(&[4, 7]);
    let chosen = call(api::CAMERA, json!({"mode":"first_person", "seat":7}));
    assert_eq!(chosen["result"]["camera"], "first-person");
    assert_eq!(chosen["result"]["camera_seat"], 7);
    assert_eq!(camera.requested_seat(), Some(7));
    assert!(call(api::CAMERA, json!({"mode":"first-person","seat":3}))["error"].is_object());
    assert_eq!(call(api::CAMERA, json!({"mode":"orbit","seat":4,"zoom":2}))["result"]["camera"], "orbit");
    assert_eq!(call(api::CAMERA, json!({"mode":"default"}))["result"]["camera"], "default");
    rendered.lock().header.meta.source = Source::Local;
    assert!(call(api::CAMERA, json!({"mode":"orbit"}))["error"].is_object());
    assert_eq!(call(api::CAMERA, json!({"mode":"first-person","seat":7}))["result"]["camera_seat"], 7);
    player.add_methods(&a);
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

#[test]
fn sealed_film_reinstalls_the_recorders_telemetry_and_impact_hook_chains() {
    use crate::telemetry::TelemetryConfig;
    use live_proto::film::TelemetryHooks;

    let plan = |session: &Arc<Session>, transport: TelemetryConfig| {
        let mut features = hooks::Registry::new(hooks::Switches::defaults());
        crate::run_plan::register_telemetry(&mut features, &transport, Some(session))
            .unwrap()
            .expect("telemetry hook plan");
        features.build(&hooks::Base).unwrap().text()
    };
    let dir = std::env::temp_dir().join(format!("film-hooks-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let opts = options(&dir);
    let guest = arena();
    for hooks in [
        TelemetryHooks::UNIFIED,
        TelemetryHooks {
            object_delete: false,
            ..TelemetryHooks::UNIFIED
        },
    ] {
        let path = dir.join(if hooks.object_delete {
            "full.h2film"
        } else {
            "slim.h2film"
        });
        let rec = Session::open(
            Some(path.clone()),
            None,
            Facts {
                telemetry_hooks: hooks,
                render_hooks: film::RenderHooks::Slim,
                ..host_facts()
            },
            b"xbe",
            &opts,
        )
        .unwrap()
        .unwrap();
        let captured = plan(&rec, TelemetryConfig::default());
        for label in [
            "match start",
            "sample",
            "kill",
            "film barrel fire",
            "film impact geometry",
        ] {
            assert!(captured.contains(label), "missing {label}");
        }
        assert_eq!(
            captured.contains("film projectile end"),
            hooks.object_delete
        );
        for _ in 0..30 {
            rec.checkpoint(&guest);
        }
        rec.finish(40, true).unwrap();
        let header = Reader::open(&path).unwrap().header;
        assert_eq!(header.telemetry_hooks, Some(hooks));
        assert_eq!(header.render_hooks, Some(film::RenderHooks::Slim));

        // A desktop module provides every original, including functions
        // the host's image did not generate. Replay keeps the sealed plan.
        let replay = |facts| {
            Session::open(
                None,
                Some(Replay {
                    path: path.clone(),
                    cache: None,
                    unpaced: false,
                    render_every: 0,
                    speed: 1.,
                    seek_tick: 0,
                    paused_at_target: false,
                    presentation: Presentation::default(),
                    restart: dir.join("restart"),
                    camera: Default::default(),
                    window_closed: None,
                }),
                facts,
                b"xbe",
                &opts,
            )
        };
        let desktop = replay(Facts {
            render_hooks: film::RenderHooks::Gpu,
            ..host_facts()
        }).unwrap().unwrap();
        assert_eq!(desktop.render_hooks(), film::RenderHooks::Slim);
        assert_eq!(captured, plan(&desktop, TelemetryConfig::playback()));
        let missing_damage = Facts {
            telemetry_hooks: TelemetryHooks {
                damage_report: false,
                ..hooks
            },
            ..host_facts()
        };
        assert!(
            replay(missing_damage).is_err_and(|e| e.to_string().contains("film telemetry plan"))
        );
    }
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
        telemetry_hooks: live_proto::film::TelemetryHooks::UNIFIED,
        render_hooks: crate::run_plan::compiled_render_hooks(),
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

#[test]
fn malformed_viewer_observations_leave_a_seekable_film_and_count_at_seal() {
    use live_proto::{
        film::{
            objects::{HillPose, Scene},
            stream,
        },
        game::{Frame, FramePlayer},
    };
    let dir = std::env::temp_dir().join(format!("film-dropped-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("observations.h2film");
    let session = Session::open(
        Some(path.clone()),
        None,
        host_facts(),
        b"xbe",
        &options(&dir),
    )
    .unwrap()
    .unwrap();
    let observer = session.film_observer().unwrap();
    let mut guest = arena();
    let engine = engine::objectives::GAME_ENGINE;
    if guest.u32(engine).is_err() {
        guest
            .map_zeroed(engine & !4095, 4096, Permissions::RW)
            .unwrap();
    }
    guest.set_u32(engine, 0).unwrap();
    let hill = |x| Scene {
        missing: Default::default(),
        hill: Some(HillPose {
            index: 2,
            corners: vec![[x, 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
        }),
        objects: Vec::new(),
    };
    let player = |tick, yaw| Frame {
        game_id: "fixture".into(),
        tick,
        players: vec![FramePlayer {
            index: 0,
            alive: true,
            yaw,
            ..Default::default()
        }],
        ..Default::default()
    };
    observer.clock(Some(1));
    observer.scene(hill(0.)).unwrap();
    observer.frame(player(1, 0.)).unwrap();
    session.checkpoint(&guest);

    guest.set_u32(engine, 0x3010_0000).unwrap(); // unavailable objective registry
    observer.clock(Some(2));
    observer
        .event(
            &crate::telemetry::GameEvent::new(
                "fixture",
                live_proto::game::PLAYER_KILLED,
                json!({"victim": "unreadable"}),
            ),
            2,
        )
        .unwrap();
    observer
        .record(live_proto::film::events::Record {
            tick: 2,
            event: live_proto::film::events::Event::Pickup {
                unit: "a".repeat(live_proto::film::events::MAX_EVENT_BYTES),
            },
        })
        .unwrap();
    observer.scene(hill(f64::NAN)).unwrap();
    observer.frame(player(2, f64::INFINITY)).unwrap();
    session.checkpoint(&guest);
    assert!(observer.active());

    guest.set_u32(engine, 0).unwrap();
    observer.clock(Some(3));
    observer.scene(hill(2.)).unwrap();
    observer.frame(player(3, 1.)).unwrap();
    session.checkpoint(&guest);
    // A bad final observation still belongs in the sealed summary.
    observer.scene(hill(f64::INFINITY)).unwrap();
    session.finish(3, true).unwrap();

    let mut range = film::range::RangeReader::new(File::open(&path).unwrap()).unwrap();
    assert_eq!(
        range.front.summary.dropped_samples,
        [
            ("unit_frame".to_owned(), 1),
            ("highlight_encoding".to_owned(), 1),
            ("stats_event".to_owned(), 1),
            ("objective".to_owned(), 1),
            ("hill".to_owned(), 2),
        ]
        .into_iter()
        .collect()
    );
    let mut poses = film::presentation::Decoder::default();
    let mut observed = Vec::new();
    for i in 0..range.directory.chunks.len() {
        let bytes = range.chunk(i).unwrap();
        for record in stream::Chunk::parse(&bytes).unwrap().records() {
            let record = record.unwrap();
            if record.tag == stream::POSE {
                observed.push(record.tick);
                poses.apply(record.payload).unwrap();
            }
        }
    }
    assert!(observed.contains(&1) && !observed.contains(&2) && observed.contains(&3));
    assert!((poses.scene().hill.unwrap().corners[0][0] - 2.0).abs() < 0.002);
    assert_eq!(poses.extras().unwrap().tick, 3);
    let mut replay = film::blam::TrackReader::open(path).unwrap();
    for tick in 1..=3 {
        assert_eq!(replay.step().unwrap().unwrap().tick, tick);
    }
    assert!(replay.step().unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

mod medal_recording;
