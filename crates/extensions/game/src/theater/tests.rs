mod capture_points;
mod media;
mod pad_inputs;
mod transport;
mod sidecar_restore;
mod camera_redraw;
mod replay_cursor;

use super::*;
use agent_proto::theater as api;
use runtime::guest::Permissions;
use serde_json::json;

pub(super) fn arena() -> runtime::Guest {
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
                verification: None,
                path: file.clone(),
                cache: None,
                unpaced,
                render_every,
                speed: 1.,
                seek_tick: 0,
                from_tick: None,
                seek_source: None,
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
    // Exercise the public wire response while holding a synthetic rewind.
    let rewound = replay();
    rewound.add_methods(&a);
    {
        let mut state = rewound.lock();
        state.ring_status = RingStatus { ticks: vec![5, 10], bytes: 12_288, cap: 16_384 };
        state.seek = Some(SeekProgress { target: 20, from_tick: 40, source: api::SeekSource::Ring,
            requested_source: None, warm: true, restore_ms: Some(7) });
        state.tick = 10;
    }
    let inventory = &call(api::KEYFRAMES, json!({}))["result"];
    assert_eq!(inventory["ring"], json!([5, 10]));
    assert_eq!(inventory["ring_bytes"], 12_288);
    assert_eq!(inventory["ring_cap"], 16_384);
    assert!(call(api::KEYFRAMES, json!({"unknown": 1}))["error"].is_object());
    let seek = &call(api::STATE, json!({}))["result"]["seek"];
    assert_eq!(seek, &json!({
        "from_tick": 40, "target": 20, "warm": true, "source": "ring",
        "remaining_ticks": 10, "restore_ms": 7,
    }));
    player.add_methods(&a);
    assert_eq!(call(api::STATE, json!({}))["result"]["compatibility"], "playable");
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
    let eyes = [4, 7].map(|seat| camera::ObserverEye {
        seat,
        position: [8., 1., 2.],
        aim: [1., 0., 0.],
        facing: [1., 0., 0.],
        fov: 85.,
    });
    camera.host_pose(eyes.into_iter()).unwrap();
    let chosen = call(api::CAMERA, json!({"mode":"first_person", "seat":7, "fov":82}));
    assert_eq!(chosen["result"]["camera"], "first-person");
    assert_eq!(chosen["result"]["camera_seat"], 7);
    assert_eq!(chosen["result"]["camera_pose"]["fov"], 82.);
    assert!(call(api::CAMERA, json!({"mode":"first-person","seat":3}))["error"].is_object());
    assert_eq!(call(api::CAMERA, json!({"mode":"orbit","seat":4,"zoom":2}))["result"]["camera"], "orbit");
    assert_eq!(call(api::CAMERA, json!({"mode":"default"}))["result"]["camera"], "default");
    assert!(call(api::CAMERA, json!({"mode":"orbit","fov":180}))["error"].is_object());
    let hud = call(api::HUD, json!({"game":false,"bar":false}));
    assert_eq!(hud["result"]["hud"], false);
    assert_eq!(hud["result"]["bar"], false);
    let bar = call(api::HUD, json!({"bar":true}));
    assert_eq!(bar["result"]["hud"], false);
    assert_eq!(bar["result"]["bar"], true);

    let local = open(facts(), 1, false).unwrap().unwrap();
    local.lock().header.meta.source = Source::Local;
    local.add_methods(&a);
    assert_eq!(call(api::CAMERA, json!({"mode":"orbit"}))["result"]["camera"], "orbit");
    let remote = call(api::CAMERA, json!({"mode":"first-person","seat":7}));
    assert_eq!(remote["result"]["camera_seat"], 7);
    assert_eq!(remote["result"]["camera"], "follow");
    assert_eq!(remote["result"]["camera_fallback"], "no-recorded-viewport");
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
        let switches = hooks::Switches::parse(&header.features).unwrap();
        assert!(switches.feature(crate::replication_admission::FEATURE, false));

        // A desktop module provides every original, including functions
        // the host's image did not generate. Replay keeps the sealed plan.
        let replay = |facts| {
            Session::open(
                None,
                Some(Replay {
                    verification: None,
                    path: path.clone(),
                    cache: None,
                    unpaced: false,
                    render_every: 0,
                    speed: 1.,
                    seek_tick: 0,
                    from_tick: None,
                    seek_source: None,
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
        assert!(desktop.uses_replication_admission());
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

pub(super) fn host_facts() -> Facts {
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
        player_features: false,
    }
}

pub(super) fn options(data: &std::path::Path) -> crate::RunOpts {
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
        // The stats stream, then the film file beside it.
        let mut bodies = Vec::new();
        for target in [
            "POST /v1/games/hl-fixture/film HTTP/1.1",
            "POST /v1/games/hl-fixture/film?object=film HTTP/1.1",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), target);
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
            drop(reader);
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            bodies.push(body);
        }
        let film_bytes = bodies.pop().unwrap();
        let uploaded = Reader::new(Cursor::new(bodies.pop().unwrap())).unwrap();
        assert!(uploaded.finalized);
        assert!(uploaded.header.inputs_in_film);
        assert_eq!(uploaded.header.meta.map, "lockout");
        assert_eq!(uploaded.header.meta.length_ticks, 30);
        assert!(film::file::FilmReader::open(Cursor::new(film_bytes)).unwrap().sealed);
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
fn state_reports_the_seek_route_and_restore_cost_and_lists_keyframes() {
    let dir = std::env::temp_dir().join(format!("engine-seek-state-{}", live_proto::sign::random_id()));
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
    let snapshot = crate::snapshot_glue::save_parts(
        &mut machine.parts,
        snapshot::Header {
            build: machine.shelf.build().clone(),
            name: "film-30".into(),
            tick: 30,
            presents: 1,
            notes: vec![],
        },
        &*machine.hash,
    ).unwrap();
    if let Mode::Record(rec) = &mut record.lock().mode {
        let out = rec.out.as_mut().unwrap();
        out.chunk(film::MARK, &Mark::D3dBarrier {
            slices: 35,
            tick: 30,
            saved: true,
        }).unwrap();
        rec.track.keyframe(out, &film::blam::Keyframe {
            tick: 30,
            slices: 35,
            calls: 0,
            lobby_boundary: 0,
            snapshot,
            disk_delta: Vec::new(),
            ram_anchor: None,
        }).unwrap();
    }
    record.checkpoint(&guest);
    record.finish(40, true).unwrap();

    let build = machine.shelf.build().clone();
    let open = |seek_tick, seek_source| {
        let player = Session::open(None, Some(Replay {
            verification: None,
            path: file.clone(),
            cache: None,
            unpaced: false,
            render_every: 0,
            speed: 1.,
            seek_tick,
            from_tick: None,
            seek_source,
            paused_at_target: false,
            presentation: Presentation::default(),
            restart: dir.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }), host_facts(), b"xbe", &opts).unwrap().unwrap();
        player.mount_sidecar(&dir, 0, &build, &dir).unwrap();
        player
    };
    let a = agent::Agent::new();
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
    // A cold seek to tick 31 routes through the film's keyframe at tick 30.
    let player = open(31, Some(api::SeekRoute::Keyframe));
    player.add_methods(&a);
    let seek = call(api::STATE, json!({}))["result"]["seek"].clone();
    assert_eq!(seek["target"], 31);
    assert_eq!(seek["from_tick"], 0);
    assert_eq!(seek["source"], "keyframe");
    assert_eq!(seek["warm"], false);
    assert_eq!(seek["remaining_ticks"], 31);
    assert!(seek["restore_ms"].is_null());
    let stored = call(api::KEYFRAMES, json!({}))["result"].clone();
    assert_eq!(stored["film"], json!([30]));
    assert_eq!(stored["capture_points"], json!([30]));
    assert_eq!(stored["ring"], json!([]));
    assert_eq!(stored["sidecar"], json!([]));
    assert_eq!(stored["ring_bytes"], 0);
    // The ring is on by default at its 256 MiB budget.
    assert_eq!(stored["ring_cap"], 256u64 << 20);
    assert_eq!(stored["sidecar_bytes"], 0);
    assert!(call(api::KEYFRAMES, json!({"film":true}))["error"].is_object());

    // The restore reports its cost and the shrinking distance to the target.
    let keyframe = player.take_keyframe().unwrap();
    player.restore_keyframe(keyframe, &mut machine, None).unwrap();
    let start = std::time::Instant::now();
    player.start_measurements(start, Some(start));
    player.measure_first_slice();
    let seek = call(api::STATE, json!({}))["result"]["seek"].clone();
    assert_eq!(seek["remaining_ticks"], 1);
    assert!(seek["restore_ms"].is_u64(), "{seek}");
    let measured = call(api::MEASUREMENTS, json!({}))["result"].clone();
    assert!(measured["child_to_first_slice_ms"].is_u64());
    assert!(measured["fast_forward_ms"].is_null());
    player.checkpoint(&guest);
    let seek = call(api::STATE, json!({}))["result"]["seek"].clone();
    assert_eq!(seek["remaining_ticks"], 0);
    assert!(seek["restore_ms"].is_u64(), "{seek}");
    let measured = call(api::MEASUREMENTS, json!({}))["result"].clone();
    assert_eq!(measured["fast_forward_ticks"], 1);
    assert!(measured["fast_forward_ms"].is_u64());
    assert!(measured["boot_to_match_start_ms"].is_null());

    // A requested jump names its own route: covered by the tick-30 keyframe
    // or not. The tick it leaves becomes the next jump's from_tick. The film
    // ends at tick 31, so that is the only target past the keyframe.
    let jump = call(api::SEEK, json!({"tick":31}))["result"]["seek"].clone();
    assert_eq!(jump["from_tick"], 31);
    assert_eq!(jump["source"], "keyframe");
    let jump = call(api::SEEK, json!({"tick":5}))["result"]["seek"].clone();
    assert_eq!(jump["source"], "boot");

    // A target before every stored keyframe replays from boot.
    let booted = open(20, None);
    booted.add_methods(&a);
    assert!(booted.take_keyframe().is_none());
    let seek = call(api::STATE, json!({}))["result"]["seek"].clone();
    assert_eq!(seek["source"], "boot");
    assert_eq!(seek["remaining_ticks"], 20);
    assert!(seek["restore_ms"].is_null());

    // Playing from the start reports no jump.
    let forced_boot = open(31, Some(api::SeekRoute::Boot));
    assert!(forced_boot.take_keyframe().is_none());
    forced_boot.add_methods(&a);
    assert_eq!(call(api::STATE, json!({}))["result"]["seek"]["source"], "boot");

    let warm_player = open(31, None);
    let frame = warm_player.take_keyframe().unwrap();
    warm_player.restore_keyframe(frame, &mut machine, None).unwrap();
    warm_player.checkpoint(&guest);
    warm_player.finish(40, true).unwrap();
    let tracker = film::manifest::Manifest::capture(&dir).unwrap().disk_tracker(&dir);
    let warm = warm::Warm::new(96 << 20, tracker);
    warm_player.lock().ring_status = RingStatus::from_warm(Some(&warm));
    *warm_player.warm.lock().unwrap() = Some(warm);
    warm_player.lock().restart = Some(31);
    assert_eq!(warm_player.try_warm_seek(31, &mut machine, None), Some((35, 0)));
    let seek = warm_player.lock().seek_info().unwrap();
    assert!(seek.warm);
    assert_eq!(seek.source, api::SeekSource::Keyframe);
    assert_eq!(seek.remaining_ticks, 1);
    warm_player.checkpoint(&guest);
    warm_player.finish(40, true).unwrap();

    let linear = open(0, None);
    linear.add_methods(&a);
    assert!(call(api::STATE, json!({}))["result"]["seek"].is_null());
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
    // A host's actions are in the film file beside its stats stream.
    let mut replay = film::blam::TrackReader::open_with_film(path).unwrap();
    for tick in 1..=3 {
        assert_eq!(replay.step().unwrap().unwrap().tick, tick);
    }
    assert!(replay.step().unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

mod medal_recording;

#[test]
fn verification_reports_only_requested_ticks_and_returns_the_first_mismatch() {
    let dir = std::env::temp_dir().join(format!("film-verify-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let film = dir.join("fixture.h2film");
    let report = dir.join("report.json");
    let opts = options(&dir);
    let g = arena();
    let rec = Session::open(Some(film.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap().unwrap();
    for _ in 0..60 { rec.checkpoint(&g); }
    rec.finish(90, true).unwrap();
    let open = || {
        Session::open(None, Some(Replay {
            verification: Some(VerificationMode::Worker {
                baseline: None,
                request: api::Verify { from: Some(30), to: Some(60), ring: true },
                report: report.clone(),
            }),
            path: film.clone(),
            cache: None,
            unpaced: false,
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            from_tick: None,
            seek_source: None,
            paused_at_target: false,
            presentation: Default::default(),
            restart: dir.join("restart.json"),
            camera: Default::default(),
            window_closed: None,
        }), host_facts(), b"xbe", &opts).unwrap().unwrap()
    };
    let result = || serde_json::from_reader::<_, api::VerificationReport>(
        File::open(&report).unwrap()
    ).unwrap();
    let player = open();
    for _ in 0..60 { player.checkpoint(&g); }
    player.finish(90, true).unwrap();
    let clean = result();
    assert_eq!((clean.ticks, clean.arena_hashes_checked), (31, 2));
    assert!(clean.first_divergence.is_none());

    let player = open();
    assert!(!player.verify_ring_at(29));
    assert!(player.verify_ring_at(30));
    for _ in 0..30 { player.checkpoint(&g); }
    player.verified_ring_restore(30, Err(anyhow::anyhow!("fixture RAM mismatch")));
    player.verified_ring_restore(30, Err(anyhow::anyhow!("later error")));
    assert!(player.finish(40, false).is_err());
    let mismatch = result();
    assert_eq!((mismatch.ticks, mismatch.arena_hashes_checked), (1, 1));
    let first = mismatch.first_divergence.unwrap();
    assert_eq!(first.tick, 30);
    assert!(first.kind.contains("fixture RAM mismatch"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn camera_rig_survives_the_seek_presentation_round_trip() {
    let camera = FilmCamera::default();
    let eyes = [camera::ObserverEye {
        seat: 5,
        position: [12., 5., 1.5],
        aim: [1., 0., 0.],
        facing: [0., 1., 0.],
        fov: 70.,
    }];
    camera.control(api::Camera { mode: api::CameraMode::FirstPerson, ..Default::default() }, Some(5)).unwrap();
    let first = camera.host_pose(eyes.into_iter()).unwrap();
    assert_eq!(first.position, eyes[0].position);
    assert_eq!(first.forward, eyes[0].aim);
    assert_eq!(first.hide_subject, Some(5));
    camera.control(api::Camera {
        mode: api::CameraMode::Orbit,
        fov: Some(91.),
        look: [0.2, -0.28],
        zoom: 3.,
        ..Default::default()
    }, Some(5)).unwrap();
    camera.host_pose(eyes.into_iter()).unwrap();
    camera.control(api::Camera {
        mode: api::CameraMode::Free,
        movement: [1., 2., 3.],
        ..Default::default()
    }, Some(5)).unwrap();
    camera.set_hud(false);
    let before = camera.state();
    let view = before.pose.unwrap();
    assert_eq!(view.fov, 91.);
    assert_eq!(view.radius, 7.);
    let mut carried = Presentation { bar: false, ..Default::default() };
    camera.save(&mut carried);

    // The CLI passes this same wire state to the child after a seek request.
    let restart = Restart {
        target: 90,
        from_tick: Some(30),
        source: None,
        speed: 0.5,
        paused: true,
        presentation: carried,
    };
    let wire = serde_json::to_vec(&restart).unwrap();
    let reopened_restart: Restart = serde_json::from_slice(&wire).unwrap();
    assert_eq!((reopened_restart.target, reopened_restart.speed, reopened_restart.paused), (90, 0.5, true));
    let carried = reopened_restart.presentation;
    assert!(!carried.hud && !carried.bar);
    let reopened = FilmCamera::default();
    reopened.restore(&carried);
    assert_eq!(reopened.state(), before);
    let next = reopened.host_pose(std::iter::empty()).unwrap();
    assert_eq!(next.position, view.position);
    assert_eq!(next.fov, crate::spectator_view::vertical_fov(91.));
}

#[test]
fn camera_motion_and_display_lod_leave_film_and_netgame_digests_unchanged() {
    let dir = std::env::temp_dir().join(format!("film-camera-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("synthetic.h2film");
    let opts = options(&dir);
    let rec = Session::open(Some(path.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap().unwrap();
    let mut guest = arena();
    const SEED: u32 = 0x800000;
    let pointer = crate::sim_glue::RANDOM_SEED_PTR;
    guest.map_zeroed(pointer & !4095, 4096, Permissions::RW).unwrap();
    guest.set_u32(pointer, SEED).unwrap();
    #[cfg(feature = "gpu")]
    let change_lod = |guest: &mut runtime::Guest, highest| {
        use render_bridge::native_scene_lists::{LodDetail, ViewLodScale};
        const VIEW_LOD: u32 = 0x10000;
        let scale = ViewLodScale::new(VIEW_LOD + 4);
        let display = if highest { LodDetail::Highest } else { LodDetail::Retail };
        // A protected session's recorded geometry, and the local single-copy
        // case with no reference at all: both keep the guest word — the walk
        // the digests hash — at retail whatever the display choice.
        crate::lod_distance::reset_view(guest, VIEW_LOD, 0x0003_1ef5, display, Some(1.0)).unwrap();
        assert_eq!(scale.display(guest), display);
        assert_eq!(scale.read(guest), LodDetail::Retail);
        crate::lod_distance::reset_view(guest, VIEW_LOD, 0x0003_1ef5, display, None).unwrap();
        assert_eq!(scale.display(guest), display);
        assert_eq!(scale.read(guest), LodDetail::Retail);
    };
    #[cfg(feature = "gpu")]
    guest.map_zeroed(0x10000, 4096, Permissions::RW).unwrap();
    for tick in 1..=60 {
        guest.set_u32(SEED, tick).unwrap();
        #[cfg(feature = "gpu")]
        change_lod(&mut guest, false);
        rec.checkpoint(&guest);
    }
    rec.finish(90, true).unwrap();

    for moving_camera in [false, true] {
        let camera = FilmCamera::default();
        let player = Session::open(None, Some(Replay {
            verification: None,
            cache: None,
            unpaced: false,
            from_tick: None,
            seek_source: None,
            path: path.clone(),
            render_every: 1,
            speed: 1.,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Presentation::default(),
            restart: dir.join("restart"),
            camera: camera.clone(),
            window_closed: None,
        }), host_facts(), b"xbe", &opts).unwrap().unwrap();
        let eyes = [camera::ObserverEye {
            seat: 0, position: [1., 2., 3.], aim: [1., 0., 0.],
            facing: [1., 0., 0.], fov: 70.,
        }];
        camera.host_pose(eyes.into_iter()).unwrap();
        let initial_pose = camera.state().pose;
        for tick in 1..=60 {
            guest.set_u32(SEED, tick).unwrap();
            if moving_camera {
                camera.control(api::Camera {
                    mode: api::CameraMode::Free,
                    movement: [0.1, 0.2, 0.],
                    look: [0.01, 0.],
                    ..Default::default()
                }, None).unwrap();
                camera.host_pose(eyes.into_iter()).unwrap();
                camera::with_render_seeds(&mut guest, |draw| {
                    draw.set_u32(SEED, 0xfeed).unwrap();
                    draw.set_u32(SEED + 4, 0xbeef).unwrap();
                });
            }
            #[cfg(feature = "gpu")]
            {
                let before = lockstep::hash_sim(&guest, lockstep::SimRegion::HALO2).unwrap();
                change_lod(&mut guest, moving_camera);
                assert_eq!(lockstep::hash_sim(&guest, lockstep::SimRegion::HALO2), Some(before));
            }
            player.checkpoint(&guest);
        }
        assert_eq!(camera.state().pose != initial_pose, moving_camera);
        player.finish(90, true).unwrap();
        assert!(player.lock().complete);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
