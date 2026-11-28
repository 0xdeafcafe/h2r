mod media;
mod pad_inputs;
mod transport;

use super::*;
use agent_proto::theater as api;
use film::Reader;
use runtime::guest::Permissions;
use serde_json::json;
use std::path::Path;

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

/// Where the game of a film [`with_game_start`] writes builds its update.
pub(super) const FILM_UPDATE: u32 = 0xc2_0000;

fn film_options() -> Vec<u8> {
    super::game_start::tests::block(crate::level_boot::MODE_MULTIPLAYER, 1)
}

/// The game a film [`with_game_start`] writes starts from.
pub(super) fn film_game() -> runtime::Guest {
    let mut g = super::game_start::tests::game(&film_options(), [11, 12]);
    g.map_zeroed(FILM_UPDATE, 0x5000, Permissions::RW).unwrap();
    g
}

/// One tick of a film's game: the update the engine builds is the film's,
/// and the tick ends.
pub(super) fn film_tick(session: &Arc<Session>, g: &mut runtime::Guest) {
    let state = session.lock();
    super::actions::at_update(session, state, g, Ok(FILM_UPDATE));
    session.checkpoint(g);
}

/// Write the film file at `path` a replay plays from, over the one the
/// recording left: the recording's header, the start of [`film_game`] at
/// tick 1, then `ticks` ticks with no actions and an empty body.
pub(super) fn with_game_start(path: &Path, ticks: u64) {
    let header = Reader::open(film::file::stats_of(path)).unwrap().header;
    let seed = film::file::Seed { header: header.clone(), descriptor: Default::default() };
    let mut w = film::file::FilmWriter::new(std::io::Cursor::new(Vec::new()), &seed, 30).unwrap();
    let start = game_start::first_update(&film_game(), film_options(), &header, None, 1).unwrap();
    w.game_start(&start).unwrap();
    for _ in 0..ticks {
        w.push_tick(0, 0, &[None; film::columns::SEATS]).unwrap();
        w.tail(&[0; film::columns::BODY_BYTES]).unwrap();
    }
    std::fs::write(path, w.finish().unwrap().into_inner()).unwrap();
}

/// A replay of `path` with these settings and the rest at their defaults.
pub(super) fn replay_of(path: &Path, render_every: u32) -> Replay {
    Replay {
        verification: None,
        path: path.to_owned(),
        render_every,
        speed: 1.,
        seek_tick: 0,
        paused_at_target: false,
        presentation: Presentation::default(),
        camera: Default::default(),
        window_closed: None,
    }
}

#[test]
fn a_replay_registers_its_controls_reports_its_seek_and_refuses_one_beyond_the_film() {
    let dir = std::env::temp_dir().join(format!("film-boot-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("fixture.h2film");
    let facts = host_facts;
    let opts = options(&dir);
    let rec = Session::open(Some(file.clone()), None, facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let g = arena();
    for _ in 0..60 {
        rec.checkpoint(&g);
    }
    rec.finish(true).unwrap();
    with_game_start(&file, 60);
    let open = |render_every| {
        Session::open(None, Some(replay_of(&file, render_every)), facts(), b"xbe", &opts)
    };
    let replay = || open(0).unwrap().unwrap();
    assert!(replay().finish(true).is_err_and(|e| {
        e.to_string().contains("playback stopped before the end")
    }));
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
    // The public wire response while a start-route jump is on its way.
    let rewound = replay();
    rewound.add_methods(&a);
    {
        let mut state = rewound.lock();
        state.seek = Some(SeekProgress {
            target: 20,
            from_tick: 40,
            source: api::SeekSource::Boot,
            requested_source: None,
        });
        state.tick = 10;
    }
    let seek = &call(api::STATE, json!({}))["result"]["seek"];
    assert_eq!(seek, &json!({
        "from_tick": 40, "target": 20, "source": "boot", "remaining_ticks": 10,
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
    assert!(call(api::SEEK, json!({"tick":61}))["error"].is_object());
    assert_eq!(
        call(api::CAMERA, json!({"mode":"free"}))["error"]["message"],
        "playback has no rendered view"
    );
    let rendered = open(1).unwrap().unwrap();
    rendered.add_methods(&a);
    let camera = match &rendered.lock().mode {
        Mode::Replay(replay) => replay.camera.clone(),
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

    let local = open(1).unwrap().unwrap();
    local.lock().header.meta.source = Source::Local;
    local.add_methods(&a);
    assert_eq!(call(api::CAMERA, json!({"mode":"orbit"}))["result"]["camera"], "orbit");
    let remote = call(api::CAMERA, json!({"mode":"first-person","seat":7}));
    assert_eq!(remote["result"]["camera_seat"], 7);
    assert_eq!(remote["result"]["camera"], "follow");
    assert_eq!(remote["result"]["camera_fallback"], "no-recorded-viewport");
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
        rec.finish(true).unwrap();
        let header = Reader::open(film::file::stats_of(&path)).unwrap().header;
        assert_eq!(header.telemetry_hooks, Some(hooks));
        assert_eq!(header.render_hooks, Some(film::RenderHooks::Slim));
        let switches = hooks::Switches::parse(&header.features).unwrap();
        assert!(switches.feature(crate::replication_admission::FEATURE, false));

        // A desktop module provides every original, including functions
        // the host's image did not generate. Replay keeps the sealed plan.
        with_game_start(&path, 30);
        let replay = |facts| Session::open(None, Some(replay_of(&path, 0)), facts, b"xbe", &opts);
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
    assert!(film.finish(true).is_err_and(|e| e.to_string().contains("fixture publication failed")));
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
    session.finish(false).unwrap();
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
    session.finish(false).unwrap();
    assert!(session.upload.lock().unwrap().is_some());
    session.finish(true).unwrap();
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
    rec.finish(true).unwrap();

    // The actions are in the film file, the events in its stats container.
    let mut track = TrackReader::open_with_film(&file).unwrap();
    let first = track.step().unwrap().unwrap();
    assert_eq!(first.inputs, vec![action]);
    assert_eq!(first.events, vec![Event::Pause { paused: true }, Event::Pause { paused: false }]);
    let next = track.step().unwrap().unwrap();
    assert!(next.inputs.is_empty());
    assert!(next.events.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

/// More simulation updates than one tick can hold fail the recording, not
/// the process.
#[test]
fn more_updates_than_a_tick_can_hold_fail_the_recording_not_the_process() {
    let dir = std::env::temp_dir().join(format!("many-updates-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let rec = Session::open(Some(dir.join("many.h2film")), None, host_facts(), b"xbe", &options(&dir))
        .unwrap()
        .unwrap();
    for _ in 0..super::record::MAX_UPDATES {
        rec.record_actions(Vec::new());
    }
    assert!(rec.lock().failure.is_none());
    rec.record_actions(Vec::new());
    assert!(rec.lock().failure.is_some());
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
    session.finish(true).unwrap();

    let mut range = film::range::RangeReader::new(File::open(film::file::stats_of(&path)).unwrap()).unwrap();
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
    // A host's actions are in its film file, beside its stats container.
    let mut replay = film::blam::TrackReader::open_with_film(path).unwrap();
    for tick in 1..=3 {
        assert_eq!(replay.step().unwrap().unwrap().tick, tick);
    }
    assert!(replay.step().unwrap().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

mod medal_recording;

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

    // The CLI passes this same wire state to the player process it starts.
    let launch = Launch { target: 90, speed: 0.5, paused: true, presentation: carried };
    let wire = serde_json::to_vec(&launch).unwrap();
    let reopened_launch: Launch = serde_json::from_slice(&wire).unwrap();
    assert_eq!((reopened_launch.target, reopened_launch.speed, reopened_launch.paused), (90, 0.5, true));
    let carried = reopened_launch.presentation;
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

    let camera = FilmCamera::default();
    let eyes = [camera::ObserverEye {
        seat: 0, position: [1., 2., 3.], aim: [1., 0., 0.],
        facing: [1., 0., 0.], fov: 70.,
    }];
    camera.host_pose(eyes.into_iter()).unwrap();
    let initial_pose = camera.state().pose;
    for tick in 1..=60 {
        guest.set_u32(SEED, tick).unwrap();
        let sim = lockstep::hash_sim(&guest, lockstep::SimRegion::HALO2);
        let film = crate::sim_glue::film_gameplay_hash(&guest);
        camera.control(api::Camera {
            mode: api::CameraMode::Free,
            movement: [0.1, 0.2, 0.],
            look: [0.01, 0.],
            ..Default::default()
        }, None).unwrap();
        camera.host_pose(eyes.into_iter()).unwrap();
        // The view draws with its own random seeds; the game's come back.
        camera::with_render_seeds(&mut guest, |draw| {
            draw.set_u32(SEED, 0xfeed).unwrap();
            draw.set_u32(SEED + 4, 0xbeef).unwrap();
        });
        #[cfg(feature = "gpu")]
        change_lod(&mut guest, tick % 2 == 0);
        assert_eq!(lockstep::hash_sim(&guest, lockstep::SimRegion::HALO2), sim);
        assert_eq!(crate::sim_glue::film_gameplay_hash(&guest), film);
    }
    assert_ne!(camera.state().pose, initial_pose);
}
